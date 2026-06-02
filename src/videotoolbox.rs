use std::{
    io::{Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use crate::nvenc::{NvencCodec, NvencPacket, Packetizer};

pub struct FfmpegVideoToolboxEncoder {
    codec: NvencCodec,
    width: u32,
    height: u32,
    fps: u32,
    child: Child,
    stdin: ChildStdin,
    packets: Receiver<NvencPacket>,
}

impl FfmpegVideoToolboxEncoder {
    pub fn new(
        codec: NvencCodec,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
    ) -> Result<Self, String> {
        if !matches!(codec, NvencCodec::H264 | NvencCodec::H265) {
            return Err(format!(
                "VideoToolbox does not expose {} encoder",
                codec.label()
            ));
        }

        let width = width.max(2);
        let height = height.max(2);
        let fps = fps.clamp(5, 60);
        let mut child = Command::new("ffmpeg")
            .args(ffmpeg_videotoolbox_args(codec, width, height, fps, bitrate))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn ffmpeg VideoToolbox backend: {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg VideoToolbox stdin unavailable".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ffmpeg VideoToolbox stdout unavailable".to_owned())?;
        let (tx, packets) = mpsc::channel();
        thread::spawn(move || read_videotoolbox_packets(codec, stdout, tx));

        Ok(Self {
            codec,
            width,
            height,
            fps,
            child,
            stdin,
            packets,
        })
    }

    pub fn matches(&self, codec: NvencCodec, width: u32, height: u32, fps: u32) -> bool {
        self.codec == codec
            && self.width == width.max(2)
            && self.height == height.max(2)
            && self.fps == fps.clamp(5, 60)
    }

    pub fn encode_bgra(&mut self, bgra: &[u8]) -> Result<Option<NvencPacket>, String> {
        let expected = self.width.saturating_mul(self.height).saturating_mul(4) as usize;
        if bgra.len() < expected {
            return Err(format!(
                "BGRA frame is too small for VideoToolbox: got {}, need {expected}",
                bgra.len()
            ));
        }

        self.stdin
            .write_all(&bgra[..expected])
            .map_err(|e| format!("write raw frame to ffmpeg VideoToolbox: {e}"))?;
        self.stdin
            .flush()
            .map_err(|e| format!("flush ffmpeg VideoToolbox stdin: {e}"))?;

        match self.packets.recv_timeout(Duration::from_millis(25)) {
            Ok(packet) => Ok(Some(packet)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("ffmpeg VideoToolbox packet reader stopped".to_owned())
            }
        }
    }
}

impl Drop for FfmpegVideoToolboxEncoder {
    fn drop(&mut self) {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn videotoolbox_supported_platform() -> bool {
    cfg!(target_os = "macos")
}

pub fn ffmpeg_videotoolbox_codecs() -> Vec<NvencCodec> {
    if !videotoolbox_supported_platform() {
        return Vec::new();
    }
    let Some(encoders) = ffmpeg_videotoolbox_encoders() else {
        return Vec::new();
    };
    let mut codecs = Vec::new();
    if encoders
        .iter()
        .any(|encoder| encoder == "h264_videotoolbox")
    {
        codecs.push(NvencCodec::H264);
    }
    if encoders
        .iter()
        .any(|encoder| encoder == "hevc_videotoolbox")
    {
        codecs.push(NvencCodec::H265);
    }
    codecs
}

pub fn ffmpeg_videotoolbox_encoders() -> Option<Vec<String>> {
    if !videotoolbox_supported_platform() {
        return Some(Vec::new());
    }
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let mut encoders = Vec::new();
    for name in ["h264_videotoolbox", "hevc_videotoolbox"] {
        if text.contains(name) {
            encoders.push(name.to_owned());
        }
    }
    Some(encoders)
}

fn ffmpeg_videotoolbox_args(
    codec: NvencCodec,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
) -> Vec<String> {
    let size = format!("{width}x{height}");
    let fps_s = fps.to_string();
    let gop = fps.saturating_mul(2).to_string();
    let bitrate_k = format!("{}k", (bitrate / 1000).max(600));
    let mut args = vec![
        "-hide_banner".to_owned(),
        "-loglevel".to_owned(),
        "error".to_owned(),
        "-nostdin".to_owned(),
        "-f".to_owned(),
        "rawvideo".to_owned(),
        "-pix_fmt".to_owned(),
        "bgra".to_owned(),
        "-s".to_owned(),
        size,
        "-r".to_owned(),
        fps_s,
        "-i".to_owned(),
        "pipe:0".to_owned(),
        "-an".to_owned(),
        "-sn".to_owned(),
        "-dn".to_owned(),
        "-vf".to_owned(),
        "pad=ceil(iw/2)*2:ceil(ih/2)*2".to_owned(),
        "-c:v".to_owned(),
        videotoolbox_encoder(codec).to_owned(),
        "-realtime".to_owned(),
        "1".to_owned(),
        "-allow_sw".to_owned(),
        "0".to_owned(),
        "-b:v".to_owned(),
        bitrate_k.clone(),
        "-maxrate".to_owned(),
        bitrate_k.clone(),
        "-bufsize".to_owned(),
        bitrate_k,
        "-g".to_owned(),
        gop,
        "-bf".to_owned(),
        "0".to_owned(),
    ];

    args.extend([
        "-bsf:v".to_owned(),
        videotoolbox_aud_bsf(codec).to_owned(),
        "-flush_packets".to_owned(),
        "1".to_owned(),
        "-f".to_owned(),
        videotoolbox_output_format(codec).to_owned(),
        "pipe:1".to_owned(),
    ]);
    args
}

fn read_videotoolbox_packets(
    codec: NvencCodec,
    mut stdout: impl Read,
    tx: mpsc::Sender<NvencPacket>,
) {
    let mut parser = Packetizer::new(codec);
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let read = match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => break,
        };
        for packet in parser.push(&buf[..read]) {
            if tx.send(packet).is_err() {
                return;
            }
        }
    }
    for packet in parser.finish() {
        let _ = tx.send(packet);
    }
}

fn videotoolbox_encoder(codec: NvencCodec) -> &'static str {
    match codec {
        NvencCodec::H264 => "h264_videotoolbox",
        NvencCodec::H265 => "hevc_videotoolbox",
        NvencCodec::Av1 => "av1_videotoolbox",
    }
}

fn videotoolbox_output_format(codec: NvencCodec) -> &'static str {
    match codec {
        NvencCodec::H264 => "h264",
        NvencCodec::H265 => "hevc",
        NvencCodec::Av1 => "ivf",
    }
}

fn videotoolbox_aud_bsf(codec: NvencCodec) -> &'static str {
    match codec {
        NvencCodec::H264 => "h264_metadata=aud=insert",
        NvencCodec::H265 => "hevc_metadata=aud=insert",
        NvencCodec::Av1 => "av1_metadata",
    }
}
