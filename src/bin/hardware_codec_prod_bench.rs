use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use evertydesk_core::mf_encode::{mf_encoder_status, MfVideoEncoder};
use evertydesk_core::mf_video::{mf_video_decode_status, MfVideoCodec, MfVideoDecoder};
use evertydesk_core::nvenc::NvencCodec;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File};
use std::hint::black_box;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TILE_SIZE: usize = 32;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum DirtyDistribution {
    Clustered,
    Scattered,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum DirtyEntropy {
    Invert,
    Noise,
}

#[derive(Debug)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

#[derive(Debug, Clone)]
struct Config {
    width: usize,
    height: usize,
    fps: u32,
    bitrate: u32,
    iterations: usize,
    warmup: usize,
    out_dir: PathBuf,
    codecs: Vec<NvencCodec>,
}

impl Config {
    fn parse() -> Result<Self, String> {
        let mut width = 1920usize;
        let mut height = 1080usize;
        let mut fps = 60u32;
        let mut bitrate = 8_000_000u32;
        let mut iterations = 300usize;
        let mut warmup = 30usize;
        let mut out_dir: Option<PathBuf> = None;
        let mut codecs = vec![NvencCodec::H264, NvencCodec::H265];
        let mut quick = false;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--width" => width = parse_next(&mut args, "--width")?,
                "--height" => height = parse_next(&mut args, "--height")?,
                "--fps" => fps = parse_next(&mut args, "--fps")?,
                "--bitrate" => bitrate = parse_next(&mut args, "--bitrate")?,
                "--iterations" => iterations = parse_next(&mut args, "--iterations")?,
                "--warmup" => warmup = parse_next(&mut args, "--warmup")?,
                "--out" => out_dir = Some(PathBuf::from(next_arg(&mut args, "--out")?)),
                "--codec" => codecs = parse_codecs(&next_arg(&mut args, "--codec")?)?,
                "--quick" => quick = true,
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }

        if quick {
            iterations = iterations.min(40);
            warmup = warmup.min(5);
        }
        if width == 0 || height == 0 {
            return Err("width/height must be positive".to_owned());
        }
        if iterations == 0 {
            return Err("iterations must be positive".to_owned());
        }
        if codecs.is_empty() {
            return Err("at least one codec must be selected".to_owned());
        }

        let out_dir = out_dir.unwrap_or_else(default_out_dir);
        Ok(Self {
            width,
            height,
            fps,
            bitrate,
            iterations,
            warmup,
            out_dir,
            codecs,
        })
    }
}

#[derive(Debug, Serialize)]
struct BenchRow {
    codec: String,
    scenario: String,
    operation: String,
    width: usize,
    height: usize,
    fps: u32,
    bitrate: u32,
    dirty_ratio_target: f32,
    dirty_distribution: DirtyDistribution,
    dirty_entropy: DirtyEntropy,
    iterations: usize,
    warmup: usize,
    raw_bytes: usize,
    payload_bytes: usize,
    packets_emitted: usize,
    available: bool,
    backend: String,
    error: String,
    mean_us: f64,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    min_us: f64,
    max_us: f64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut raw_args = env::args().skip(1);
    if matches!(raw_args.next().as_deref(), Some("--decode-child")) {
        let request_path = raw_args
            .next()
            .ok_or("--decode-child requires a request JSON path")?;
        return run_decode_child(PathBuf::from(request_path));
    }
    let mut raw_args = env::args().skip(1);
    if matches!(raw_args.next().as_deref(), Some("--roundtrip-child")) {
        let request_path = raw_args
            .next()
            .ok_or("--roundtrip-child requires a request JSON path")?;
        return run_roundtrip_child(PathBuf::from(request_path));
    }

    let config = match Config::parse() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            eprintln!();
            print_help();
            std::process::exit(2);
        }
    };

    fs::create_dir_all(&config.out_dir)?;
    let csv_path = config.out_dir.join("hardware_codec_prod_bench.csv");
    let jsonl_path = config.out_dir.join("hardware_codec_prod_bench.jsonl");
    let mut csv = BufWriter::new(File::create(&csv_path)?);
    let mut jsonl = BufWriter::new(File::create(&jsonl_path)?);

    writeln!(
        csv,
        "codec,scenario,operation,width,height,fps,bitrate,dirty_ratio_target,dirty_distribution,dirty_entropy,iterations,warmup,raw_bytes,payload_bytes,packets_emitted,available,backend,error,mean_us,p50_us,p95_us,p99_us,min_us,max_us"
    )?;

    let mut rows = Vec::new();
    for codec in &config.codecs {
        rows.extend(run_codec(*codec, &config));
    }

    for row in &rows {
        writeln!(
            csv,
            "{},{},{},{},{},{},{},{:.4},{:?},{:?},{},{},{},{},{},{},{},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}",
            row.codec,
            row.scenario,
            row.operation,
            row.width,
            row.height,
            row.fps,
            row.bitrate,
            row.dirty_ratio_target,
            row.dirty_distribution,
            row.dirty_entropy,
            row.iterations,
            row.warmup,
            row.raw_bytes,
            row.payload_bytes,
            row.packets_emitted,
            row.available,
            csv_field(&row.backend),
            csv_field(&row.error),
            row.mean_us,
            row.p50_us,
            row.p95_us,
            row.p99_us,
            row.min_us,
            row.max_us
        )?;
        writeln!(jsonl, "{}", serde_json::to_string(row)?)?;
    }

    csv.flush()?;
    jsonl.flush()?;

    println!("Hardware codec production bench complete");
    println!("encode_status={}", mf_encoder_status().label());
    println!("decode_status={}", mf_video_decode_status().label());
    println!("csv={}", csv_path.display());
    println!("jsonl={}", jsonl_path.display());
    Ok(())
}

fn run_codec(codec: NvencCodec, config: &Config) -> Vec<BenchRow> {
    let backend = mf_encoder_status().label();
    let available = codec_available(codec);
    if !available {
        let mut rows = vec![unavailable_row(
            codec,
            config,
            Scenario {
                name: "keyframe_gradient",
                dirty_ratio: 1.0,
                distribution: DirtyDistribution::Clustered,
                entropy: DirtyEntropy::Invert,
                scene: SceneKind::KeyframeGradient,
            },
            "encode",
            &backend,
        )];
        for scenario in scenarios() {
            for operation in ["encode", "decode", "roundtrip"] {
                rows.push(unavailable_row(
                    codec, config, *scenario, operation, &backend,
                ));
            }
        }
        return rows;
    }

    let base = solid_frame(config.width, config.height, [30, 30, 30, 255]);
    let mut rows = Vec::new();
    let keyframe_scenario = Scenario {
        name: "keyframe_gradient",
        dirty_ratio: 1.0,
        distribution: DirtyDistribution::Clustered,
        entropy: DirtyEntropy::Invert,
        scene: SceneKind::KeyframeGradient,
    };
    let keyframe = gradient_frame(config.width, config.height);
    rows.push(run_encode_scenario(
        codec,
        config,
        keyframe_scenario,
        &backend,
        &keyframe,
        None,
        true,
    ));
    rows.extend(run_decode_and_roundtrip_scenario(
        codec,
        config,
        keyframe_scenario,
        &backend,
        &keyframe,
        None,
        true,
    ));

    for scenario in scenarios() {
        let frame = make_frame(config, &base, *scenario);
        rows.push(run_encode_scenario(
            codec,
            config,
            *scenario,
            &backend,
            &frame,
            Some(&base),
            false,
        ));
        rows.extend(run_decode_and_roundtrip_scenario(
            codec,
            config,
            *scenario,
            &backend,
            &frame,
            Some(&base),
            false,
        ));
    }
    rows
}

fn codec_available(codec: NvencCodec) -> bool {
    let status = mf_encoder_status();
    match codec {
        NvencCodec::H264 => status.has_hardware_h264(),
        NvencCodec::H265 => status.has_hardware_h265(),
        NvencCodec::Av1 => status.has_hardware_av1(),
    }
}

fn run_decode_and_roundtrip_scenario(
    codec: NvencCodec,
    config: &Config,
    scenario: Scenario,
    backend: &str,
    frame: &[u8],
    base_frame: Option<&[u8]>,
    force_key: bool,
) -> Vec<BenchRow> {
    if mf_decode_codec(codec).is_none() {
        let reason = format!(
            "{} decode is unavailable in this benchmark build",
            codec.label()
        );
        return vec![
            operation_error_row(codec, config, scenario, "decode", backend, reason.clone()),
            operation_error_row(codec, config, scenario, "roundtrip", backend, reason),
        ];
    };

    let reference = match reference_packets(codec, config, frame, base_frame, force_key) {
        Ok(reference) => reference,
        Err(err) => {
            return vec![
                operation_error_row(codec, config, scenario, "decode", backend, err.clone()),
                operation_error_row(codec, config, scenario, "roundtrip", backend, err),
            ];
        }
    };

    vec![
        run_decode_scenario(codec, config, scenario, backend, &reference),
        run_roundtrip_scenario(
            codec, config, scenario, backend, frame, base_frame, force_key,
        ),
    ]
}

struct ReferencePackets {
    key_packet: Option<Vec<u8>>,
    frame_packet: Vec<u8>,
    decode_stream_packets: Vec<Vec<u8>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DecodeChildRequest {
    codec: String,
    width: usize,
    height: usize,
    iterations: usize,
    warmup: usize,
    preroll_packets: usize,
    packets_b64: Vec<String>,
    out_path: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
struct DecodeChildResult {
    available: bool,
    frames_decoded: usize,
    error: String,
    mean_us: f64,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    min_us: f64,
    max_us: f64,
}

#[derive(Debug, Deserialize, Serialize)]
struct RoundtripChildRequest {
    codec: String,
    width: usize,
    height: usize,
    fps: u32,
    bitrate: u32,
    iterations: usize,
    warmup: usize,
    force_key: bool,
    base_frame_b64: Option<String>,
    frame_b64: String,
    out_path: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
struct RoundtripChildResult {
    available: bool,
    packets_emitted: usize,
    payload_bytes: usize,
    error: String,
    mean_us: f64,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    min_us: f64,
    max_us: f64,
}

fn run_decode_child_process(
    codec: NvencCodec,
    config: &Config,
    reference: &ReferencePackets,
) -> Result<DecodeChildResult, String> {
    let child_dir = config.out_dir.join("decode-child");
    fs::create_dir_all(&child_dir).map_err(|err| format!("create child dir failed: {err}"))?;
    let pid = std::process::id();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let stem = format!("{}-{pid}-{stamp}", codec.label().to_ascii_lowercase());
    let request_path = child_dir.join(format!("{stem}.request.json"));
    let result_path = child_dir.join(format!("{stem}.result.json"));
    let request = DecodeChildRequest {
        codec: codec.label().to_owned(),
        width: config.width,
        height: config.height,
        iterations: config.iterations,
        warmup: config.warmup,
        preroll_packets: usize::from(reference.key_packet.is_some()),
        packets_b64: reference
            .decode_stream_packets
            .iter()
            .map(|packet| BASE64.encode(packet))
            .collect(),
        out_path: result_path.clone(),
    };
    let request_json = serde_json::to_vec_pretty(&request)
        .map_err(|err| format!("serialize child request failed: {err}"))?;
    fs::write(&request_path, request_json)
        .map_err(|err| format!("write child request failed: {err}"))?;

    let exe = env::current_exe().map_err(|err| format!("resolve current exe failed: {err}"))?;
    let output = Command::new(exe)
        .arg("--decode-child")
        .arg(&request_path)
        .output()
        .map_err(|err| format!("spawn decode child failed: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let mut reason = format!("decode child exited with status {}", output.status);
        if !stderr.is_empty() {
            reason.push_str(&format!("; stderr={stderr}"));
        }
        if !stdout.is_empty() {
            reason.push_str(&format!("; stdout={stdout}"));
        }
        return Err(reason);
    }

    let result_json =
        fs::read(&result_path).map_err(|err| format!("read child result failed: {err}"))?;
    serde_json::from_slice(&result_json).map_err(|err| format!("parse child result failed: {err}"))
}

fn child_decode_row(
    codec: NvencCodec,
    config: &Config,
    scenario: Scenario,
    backend: &str,
    reference: &ReferencePackets,
    result: DecodeChildResult,
) -> BenchRow {
    BenchRow {
        codec: codec.label().to_owned(),
        scenario: scenario.name.to_owned(),
        operation: "decode".to_owned(),
        width: config.width,
        height: config.height,
        fps: config.fps,
        bitrate: config.bitrate,
        dirty_ratio_target: scenario.dirty_ratio,
        dirty_distribution: scenario.distribution,
        dirty_entropy: scenario.entropy,
        iterations: config.iterations,
        warmup: config.warmup,
        raw_bytes: config.width * config.height * 4,
        payload_bytes: reference.frame_packet.len(),
        packets_emitted: result.frames_decoded,
        available: result.available,
        backend: backend.to_owned(),
        error: result.error,
        mean_us: result.mean_us,
        p50_us: result.p50_us,
        p95_us: result.p95_us,
        p99_us: result.p99_us,
        min_us: result.min_us,
        max_us: result.max_us,
    }
}

fn run_decode_child(request_path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let request_json = fs::read(&request_path)?;
    let request: DecodeChildRequest = serde_json::from_slice(&request_json)?;
    let result = execute_decode_child(&request);
    fs::write(&request.out_path, serde_json::to_vec_pretty(&result)?)?;
    Ok(())
}

fn execute_decode_child(request: &DecodeChildRequest) -> DecodeChildResult {
    let decode_codec = match parse_decode_child_codec(&request.codec) {
        Ok(codec) => codec,
        Err(error) => return decode_child_error(error),
    };
    let packets = match request
        .packets_b64
        .iter()
        .map(|encoded| BASE64.decode(encoded))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(packets) if !packets.is_empty() => packets,
        Ok(_) => return decode_child_error("decode request contains no packets".to_owned()),
        Err(err) => return decode_child_error(format!("decode packet base64 failed: {err}")),
    };

    let child_config = Config {
        width: request.width,
        height: request.height,
        fps: 60,
        bitrate: 0,
        iterations: request.iterations,
        warmup: request.warmup,
        out_dir: PathBuf::new(),
        codecs: Vec::new(),
    };
    let mut dec =
        match MfVideoDecoder::new(decode_codec, request.width as u32, request.height as u32) {
            Ok(dec) => dec,
            Err(err) => return decode_child_error(err),
        };
    for packet in packets.iter().take(request.preroll_packets) {
        if let Err(err) = decode_until_frame(&mut dec, std::slice::from_ref(packet), 8) {
            return decode_child_error(format!("preroll decode failed: {err}"));
        }
    }
    let mut packet_index = request.preroll_packets;
    let mut first_error = String::new();
    let mut frames_decoded = 0usize;
    let samples = measure(&child_config, || {
        let Some(packet) = packets.get(packet_index) else {
            return None;
        };
        packet_index += 1;
        let started = Instant::now();
        match decode_until_frame(&mut dec, std::slice::from_ref(packet), 8) {
            Ok(Some((_, _, rgba))) => {
                let elapsed = started.elapsed();
                frames_decoded += 1;
                black_box(rgba.len());
                Some(elapsed)
            }
            Ok(None) => {
                if first_error.is_empty() {
                    first_error = "decoder produced no frame".to_owned();
                }
                None
            }
            Err(err) => {
                if first_error.is_empty() {
                    first_error = err;
                }
                None
            }
        }
    });
    let summary = summarize(&samples);
    let available = frames_decoded > 0 && !samples.is_empty();
    DecodeChildResult {
        available,
        frames_decoded,
        error: if available {
            String::new()
        } else if packet_index
            < request
                .preroll_packets
                .saturating_add(request.warmup)
                .saturating_add(request.iterations)
        {
            format!(
                "decoder consumed only {packet_index}/{} packets",
                request
                    .preroll_packets
                    .saturating_add(request.warmup)
                    .saturating_add(request.iterations)
            )
        } else if first_error.is_empty() {
            "decoder produced no frame".to_owned()
        } else {
            first_error
        },
        mean_us: summary.mean_us,
        p50_us: summary.p50_us,
        p95_us: summary.p95_us,
        p99_us: summary.p99_us,
        min_us: summary.min_us,
        max_us: summary.max_us,
    }
}

fn parse_decode_child_codec(value: &str) -> Result<MfVideoCodec, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "h264" => Ok(MfVideoCodec::H264),
        "h265" | "hevc" => Ok(MfVideoCodec::H265),
        other => Err(format!("unsupported decode child codec: {other}")),
    }
}

fn decode_child_error(error: String) -> DecodeChildResult {
    DecodeChildResult {
        available: false,
        frames_decoded: 0,
        error,
        mean_us: 0.0,
        p50_us: 0.0,
        p95_us: 0.0,
        p99_us: 0.0,
        min_us: 0.0,
        max_us: 0.0,
    }
}

fn run_roundtrip_scenario(
    codec: NvencCodec,
    config: &Config,
    scenario: Scenario,
    backend: &str,
    frame: &[u8],
    base_frame: Option<&[u8]>,
    force_key: bool,
) -> BenchRow {
    match run_roundtrip_child_process(codec, config, frame, base_frame, force_key) {
        Ok(result) => child_roundtrip_row(codec, config, scenario, backend, result),
        Err(err) => operation_error_row(codec, config, scenario, "roundtrip", backend, err),
    }
}

fn run_roundtrip_child_process(
    codec: NvencCodec,
    config: &Config,
    frame: &[u8],
    base_frame: Option<&[u8]>,
    force_key: bool,
) -> Result<RoundtripChildResult, String> {
    let child_dir = config.out_dir.join("roundtrip-child");
    fs::create_dir_all(&child_dir).map_err(|err| format!("create child dir failed: {err}"))?;
    let pid = std::process::id();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let stem = format!("{}-{pid}-{stamp}", codec.label().to_ascii_lowercase());
    let request_path = child_dir.join(format!("{stem}.request.json"));
    let result_path = child_dir.join(format!("{stem}.result.json"));
    let request = RoundtripChildRequest {
        codec: codec.label().to_owned(),
        width: config.width,
        height: config.height,
        fps: config.fps,
        bitrate: config.bitrate,
        iterations: config.iterations,
        warmup: config.warmup,
        force_key,
        base_frame_b64: base_frame.map(|frame| BASE64.encode(frame)),
        frame_b64: BASE64.encode(frame),
        out_path: result_path.clone(),
    };
    let request_json = serde_json::to_vec_pretty(&request)
        .map_err(|err| format!("serialize roundtrip child request failed: {err}"))?;
    fs::write(&request_path, request_json)
        .map_err(|err| format!("write roundtrip child request failed: {err}"))?;

    let exe = env::current_exe().map_err(|err| format!("resolve current exe failed: {err}"))?;
    let output = Command::new(exe)
        .arg("--roundtrip-child")
        .arg(&request_path)
        .output()
        .map_err(|err| format!("spawn roundtrip child failed: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let mut reason = format!("roundtrip child exited with status {}", output.status);
        if !stderr.is_empty() {
            reason.push_str(&format!("; stderr={stderr}"));
        }
        if !stdout.is_empty() {
            reason.push_str(&format!("; stdout={stdout}"));
        }
        return Err(reason);
    }

    let result_json = fs::read(&result_path)
        .map_err(|err| format!("read roundtrip child result failed: {err}"))?;
    serde_json::from_slice(&result_json)
        .map_err(|err| format!("parse roundtrip child result failed: {err}"))
}

fn child_roundtrip_row(
    codec: NvencCodec,
    config: &Config,
    scenario: Scenario,
    backend: &str,
    result: RoundtripChildResult,
) -> BenchRow {
    BenchRow {
        codec: codec.label().to_owned(),
        scenario: scenario.name.to_owned(),
        operation: "roundtrip".to_owned(),
        width: config.width,
        height: config.height,
        fps: config.fps,
        bitrate: config.bitrate,
        dirty_ratio_target: scenario.dirty_ratio,
        dirty_distribution: scenario.distribution,
        dirty_entropy: scenario.entropy,
        iterations: config.iterations,
        warmup: config.warmup,
        raw_bytes: config.width * config.height * 4,
        payload_bytes: result.payload_bytes,
        packets_emitted: result.packets_emitted,
        available: result.available,
        backend: backend.to_owned(),
        error: result.error,
        mean_us: result.mean_us,
        p50_us: result.p50_us,
        p95_us: result.p95_us,
        p99_us: result.p99_us,
        min_us: result.min_us,
        max_us: result.max_us,
    }
}

fn run_roundtrip_child(request_path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let request_json = fs::read(&request_path)?;
    let request: RoundtripChildRequest = serde_json::from_slice(&request_json)?;
    let result = execute_roundtrip_child(&request);
    fs::write(&request.out_path, serde_json::to_vec_pretty(&result)?)?;
    Ok(())
}

fn execute_roundtrip_child(request: &RoundtripChildRequest) -> RoundtripChildResult {
    let encode_codec = match parse_roundtrip_child_codec(&request.codec) {
        Ok(codec) => codec,
        Err(error) => return roundtrip_child_error(error),
    };
    let decode_codec = match parse_decode_child_codec(&request.codec) {
        Ok(codec) => codec,
        Err(error) => return roundtrip_child_error(error),
    };
    let base_frame = match request
        .base_frame_b64
        .as_ref()
        .map(|encoded| BASE64.decode(encoded))
        .transpose()
    {
        Ok(frame) => frame,
        Err(err) => {
            return roundtrip_child_error(format!("decode base frame base64 failed: {err}"))
        }
    };
    let frame = match BASE64.decode(&request.frame_b64) {
        Ok(frame) => frame,
        Err(err) => return roundtrip_child_error(format!("decode frame base64 failed: {err}")),
    };
    let expected = request
        .width
        .saturating_mul(request.height)
        .saturating_mul(4);
    if frame.len() < expected {
        return roundtrip_child_error(format!(
            "frame too small: {} bytes, expected at least {expected}",
            frame.len()
        ));
    }
    if let Some(base) = &base_frame {
        if base.len() < expected {
            return roundtrip_child_error(format!(
                "base frame too small: {} bytes, expected at least {expected}",
                base.len()
            ));
        }
    }

    let mut enc = match MfVideoEncoder::new(
        encode_codec,
        request.width as u32,
        request.height as u32,
        request.fps,
        request.bitrate,
    ) {
        Ok(enc) => enc,
        Err(err) => return roundtrip_child_error(err),
    };
    let mut dec =
        match MfVideoDecoder::new(decode_codec, request.width as u32, request.height as u32) {
            Ok(dec) => dec,
            Err(err) => return roundtrip_child_error(err),
        };

    if let Some(base) = &base_frame {
        let key = match encode_until_packet(&mut enc, base, true, 60) {
            Ok(Some(packet)) => packet.bytes,
            Ok(None) => {
                return roundtrip_child_error("encoder produced no base key packet".to_owned())
            }
            Err(err) => return roundtrip_child_error(format!("base encode failed: {err}")),
        };
        if let Err(err) = decode_until_frame(&mut dec, std::slice::from_ref(&key), 8) {
            return roundtrip_child_error(format!("base decode failed: {err}"));
        }
    }

    let child_config = Config {
        width: request.width,
        height: request.height,
        fps: request.fps,
        bitrate: request.bitrate,
        iterations: request.iterations,
        warmup: request.warmup,
        out_dir: PathBuf::new(),
        codecs: Vec::new(),
    };
    let mut first_error = String::new();
    let mut payload_bytes = 0usize;
    let mut packets_emitted = 0usize;
    let samples = measure(&child_config, || {
        let started = Instant::now();
        let packet = match encode_until_packet(&mut enc, black_box(&frame), request.force_key, 60) {
            Ok(Some(packet)) => packet,
            Ok(None) => {
                if first_error.is_empty() {
                    first_error = "encoder produced no packet".to_owned();
                }
                return None;
            }
            Err(err) => {
                if first_error.is_empty() {
                    first_error = err;
                }
                return None;
            }
        };
        match decode_until_frame(&mut dec, std::slice::from_ref(&packet.bytes), 8) {
            Ok(Some((_, _, rgba))) => {
                let elapsed = started.elapsed();
                payload_bytes = payload_bytes.max(packet.bytes.len());
                packets_emitted += 1;
                black_box((packet.bytes.len(), rgba.len()));
                Some(elapsed)
            }
            Ok(None) => {
                if first_error.is_empty() {
                    first_error = "decoder produced no frame".to_owned();
                }
                None
            }
            Err(err) => {
                if first_error.is_empty() {
                    first_error = err;
                }
                None
            }
        }
    });
    let summary = summarize(&samples);
    let available = packets_emitted > 0 && !samples.is_empty();
    RoundtripChildResult {
        available,
        packets_emitted,
        payload_bytes,
        error: if available {
            String::new()
        } else if first_error.is_empty() {
            "roundtrip produced no frame".to_owned()
        } else {
            first_error
        },
        mean_us: summary.mean_us,
        p50_us: summary.p50_us,
        p95_us: summary.p95_us,
        p99_us: summary.p99_us,
        min_us: summary.min_us,
        max_us: summary.max_us,
    }
}

fn parse_roundtrip_child_codec(value: &str) -> Result<NvencCodec, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "h264" => Ok(NvencCodec::H264),
        "h265" | "hevc" => Ok(NvencCodec::H265),
        other => Err(format!("unsupported roundtrip child codec: {other}")),
    }
}

fn roundtrip_child_error(error: String) -> RoundtripChildResult {
    RoundtripChildResult {
        available: false,
        packets_emitted: 0,
        payload_bytes: 0,
        error,
        mean_us: 0.0,
        p50_us: 0.0,
        p95_us: 0.0,
        p99_us: 0.0,
        min_us: 0.0,
        max_us: 0.0,
    }
}

fn reference_packets(
    codec: NvencCodec,
    config: &Config,
    frame: &[u8],
    base_frame: Option<&[u8]>,
    force_key: bool,
) -> Result<ReferencePackets, String> {
    let mut enc = MfVideoEncoder::new(
        codec,
        config.width as u32,
        config.height as u32,
        config.fps,
        config.bitrate,
    )?;
    let key_packet = if let Some(base) = base_frame {
        Some(
            encode_until_packet(&mut enc, base, true, 60)?
                .ok_or_else(|| "encoder produced no base key packet".to_owned())?
                .bytes,
        )
    } else {
        None
    };
    let frame_packet = encode_until_packet(&mut enc, frame, force_key, 60)?
        .ok_or_else(|| "encoder produced no frame packet".to_owned())?
        .bytes;
    let stream_count = config.warmup.saturating_add(config.iterations).max(1);
    let mut stream_enc = MfVideoEncoder::new(
        codec,
        config.width as u32,
        config.height as u32,
        config.fps,
        config.bitrate,
    )?;
    let mut decode_stream_packets = Vec::with_capacity(stream_count);
    if let Some(base) = base_frame {
        decode_stream_packets.push(
            encode_until_packet(&mut stream_enc, base, true, 60)?
                .ok_or_else(|| "stream encoder produced no base key packet".to_owned())?
                .bytes,
        );
    }
    for idx in 0..stream_count {
        let packet = encode_until_packet(&mut stream_enc, frame, force_key && idx == 0, 60)?
            .ok_or_else(|| "stream encoder produced no frame packet".to_owned())?
            .bytes;
        decode_stream_packets.push(packet);
    }
    Ok(ReferencePackets {
        key_packet,
        frame_packet,
        decode_stream_packets,
    })
}

fn run_decode_scenario(
    codec: NvencCodec,
    config: &Config,
    scenario: Scenario,
    backend: &str,
    reference: &ReferencePackets,
) -> BenchRow {
    match run_decode_child_process(codec, config, reference) {
        Ok(result) => child_decode_row(codec, config, scenario, backend, reference, result),
        Err(err) => operation_error_row(codec, config, scenario, "decode", backend, err),
    }
}

fn decode_until_frame(
    dec: &mut MfVideoDecoder,
    packets: &[Vec<u8>],
    max_inputs: usize,
) -> Result<Option<(usize, usize, Vec<u8>)>, String> {
    for _ in 0..max_inputs {
        if let Some(frame) = dec.decode_packets(packets.iter())? {
            return Ok(Some(frame));
        }
    }
    Ok(None)
}

fn mf_decode_codec(codec: NvencCodec) -> Option<MfVideoCodec> {
    match codec {
        NvencCodec::H264 => Some(MfVideoCodec::H264),
        NvencCodec::H265 => Some(MfVideoCodec::H265),
        NvencCodec::Av1 => None,
    }
}

fn run_encode_scenario(
    codec: NvencCodec,
    config: &Config,
    scenario: Scenario,
    backend: &str,
    frame: &[u8],
    base_frame: Option<&[u8]>,
    force_key: bool,
) -> BenchRow {
    let mut payload_bytes = 0usize;
    let mut packets_emitted = 0usize;
    let mut first_error = String::new();

    let mut enc = match MfVideoEncoder::new(
        codec,
        config.width as u32,
        config.height as u32,
        config.fps,
        config.bitrate,
    ) {
        Ok(enc) => enc,
        Err(err) => {
            return errored_row(codec, config, scenario, backend, err);
        }
    };

    let samples = measure(config, || {
        if let Some(base) = base_frame {
            if let Err(err) = encode_until_packet(&mut enc, black_box(base), true, 6) {
                if first_error.is_empty() {
                    first_error = format!("base-frame encode failed: {err}");
                }
                return None;
            }
        }

        let started = Instant::now();
        let packet = encode_until_packet(&mut enc, black_box(frame), force_key, 6);
        let elapsed = started.elapsed();
        match packet {
            Ok(Some(packet)) => {
                payload_bytes = payload_bytes.max(packet.bytes.len());
                packets_emitted += 1;
                black_box((packet.bytes.len(), packet.key));
                Some(elapsed)
            }
            Ok(None) => {
                if first_error.is_empty() {
                    first_error = "encoder produced no packet".to_owned();
                }
                None
            }
            Err(err) => {
                if first_error.is_empty() {
                    first_error = err;
                }
                None
            }
        }
    });

    let summary = summarize(&samples);
    let available = packets_emitted > 0 && !samples.is_empty();
    let error = if available {
        String::new()
    } else if first_error.is_empty() {
        "encoder produced no packet".to_owned()
    } else {
        first_error
    };
    BenchRow {
        codec: codec.label().to_owned(),
        scenario: scenario.name.to_owned(),
        operation: "encode".to_owned(),
        width: config.width,
        height: config.height,
        fps: config.fps,
        bitrate: config.bitrate,
        dirty_ratio_target: scenario.dirty_ratio,
        dirty_distribution: scenario.distribution,
        dirty_entropy: scenario.entropy,
        iterations: config.iterations,
        warmup: config.warmup,
        raw_bytes: config.width * config.height * 4,
        payload_bytes,
        packets_emitted,
        available,
        backend: backend.to_owned(),
        error,
        mean_us: summary.mean_us,
        p50_us: summary.p50_us,
        p95_us: summary.p95_us,
        p99_us: summary.p99_us,
        min_us: summary.min_us,
        max_us: summary.max_us,
    }
}

fn encode_until_packet(
    enc: &mut MfVideoEncoder,
    frame: &[u8],
    force_key: bool,
    max_inputs: usize,
) -> Result<Option<evertydesk_core::nvenc::NvencPacket>, String> {
    for attempt in 0..max_inputs {
        if let Some(packet) = enc.encode_bgra(frame, force_key && attempt == 0)? {
            return Ok(Some(packet));
        }
    }
    Ok(None)
}

fn unavailable_row(
    codec: NvencCodec,
    config: &Config,
    scenario: Scenario,
    operation: &str,
    backend: &str,
) -> BenchRow {
    BenchRow {
        codec: codec.label().to_owned(),
        scenario: scenario.name.to_owned(),
        operation: operation.to_owned(),
        width: config.width,
        height: config.height,
        fps: config.fps,
        bitrate: config.bitrate,
        dirty_ratio_target: scenario.dirty_ratio,
        dirty_distribution: scenario.distribution,
        dirty_entropy: scenario.entropy,
        iterations: config.iterations,
        warmup: config.warmup,
        raw_bytes: config.width * config.height * 4,
        payload_bytes: 0,
        packets_emitted: 0,
        available: false,
        backend: backend.to_owned(),
        error: "hardware Media Foundation encoder unavailable".to_owned(),
        mean_us: 0.0,
        p50_us: 0.0,
        p95_us: 0.0,
        p99_us: 0.0,
        min_us: 0.0,
        max_us: 0.0,
    }
}

fn operation_error_row(
    codec: NvencCodec,
    config: &Config,
    scenario: Scenario,
    operation: &str,
    backend: &str,
    error: String,
) -> BenchRow {
    let mut row = unavailable_row(codec, config, scenario, operation, backend);
    row.error = error;
    row
}

fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn errored_row(
    codec: NvencCodec,
    config: &Config,
    scenario: Scenario,
    backend: &str,
    error: String,
) -> BenchRow {
    let mut row = unavailable_row(codec, config, scenario, "encode", backend);
    row.error = error;
    row
}

#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    dirty_ratio: f32,
    distribution: DirtyDistribution,
    entropy: DirtyEntropy,
    scene: SceneKind,
}

#[derive(Clone, Copy)]
enum SceneKind {
    KeyframeGradient,
    SyntheticDirty,
    IdeTyping,
    BrowserScroll,
    TerminalScroll,
}

fn scenarios() -> &'static [Scenario] {
    &[
        Scenario {
            name: "static_0pct",
            dirty_ratio: 0.00,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "clustered_invert_5pct",
            dirty_ratio: 0.05,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "clustered_invert_15pct",
            dirty_ratio: 0.15,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "clustered_invert_50pct",
            dirty_ratio: 0.50,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "clustered_invert_90pct",
            dirty_ratio: 0.90,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_invert_5pct",
            dirty_ratio: 0.05,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_invert_15pct",
            dirty_ratio: 0.15,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_invert_50pct",
            dirty_ratio: 0.50,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_invert_90pct",
            dirty_ratio: 0.90,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_noise_5pct",
            dirty_ratio: 0.05,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_noise_15pct",
            dirty_ratio: 0.15,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_noise_50pct",
            dirty_ratio: 0.50,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_noise_90pct",
            dirty_ratio: 0.90,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "ide_typing_realistic",
            dirty_ratio: 0.01,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::IdeTyping,
        },
        Scenario {
            name: "browser_scroll_realistic",
            dirty_ratio: 0.08,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::BrowserScroll,
        },
        Scenario {
            name: "terminal_scroll_realistic",
            dirty_ratio: 0.04,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::TerminalScroll,
        },
    ]
}

fn make_frame(config: &Config, base: &[u8], scenario: Scenario) -> Vec<u8> {
    match scenario.scene {
        SceneKind::KeyframeGradient => gradient_frame(config.width, config.height),
        SceneKind::SyntheticDirty => dirty_frame(
            base,
            config.width,
            config.height,
            scenario.dirty_ratio,
            scenario.distribution,
            scenario.entropy,
        ),
        SceneKind::IdeTyping => ide_typing_scene(config.width, config.height),
        SceneKind::BrowserScroll => browser_scroll_scene(config.width, config.height),
        SceneKind::TerminalScroll => terminal_scroll_scene(config.width, config.height),
    }
}

fn measure<F>(config: &Config, mut f: F) -> Vec<Duration>
where
    F: FnMut() -> Option<Duration>,
{
    for _ in 0..config.warmup {
        black_box(f());
    }

    let mut samples = Vec::with_capacity(config.iterations);
    for _ in 0..config.iterations {
        if let Some(sample) = f() {
            samples.push(sample);
        }
    }
    samples
}

struct Summary {
    mean_us: f64,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    min_us: f64,
    max_us: f64,
}

fn summarize(samples: &[Duration]) -> Summary {
    let mut micros: Vec<f64> = samples
        .iter()
        .map(|duration| duration.as_secs_f64() * 1_000_000.0)
        .collect();
    micros.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean_us = if micros.is_empty() {
        0.0
    } else {
        micros.iter().sum::<f64>() / micros.len() as f64
    };
    Summary {
        mean_us,
        p50_us: percentile(&micros, 50.0),
        p95_us: percentile(&micros, 95.0),
        p99_us: percentile(&micros, 99.0),
        min_us: *micros.first().unwrap_or(&0.0),
        max_us: *micros.last().unwrap_or(&0.0),
    }
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((percentile / 100.0) * (sorted.len().saturating_sub(1)) as f64).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn solid_frame(w: usize, h: usize, color: [u8; 4]) -> Vec<u8> {
    color.iter().cycle().take(w * h * 4).copied().collect()
}

fn gradient_frame(w: usize, h: usize) -> Vec<u8> {
    (0..h)
        .flat_map(|y| {
            (0..w).flat_map(move |x| {
                let r = ((x * 255) / w) as u8;
                let g = ((y * 255) / h) as u8;
                let b = (((x + y) * 255) / (w + h)) as u8;
                [b, g, r, 255u8]
            })
        })
        .collect()
}

fn dirty_frame(
    base: &[u8],
    w: usize,
    h: usize,
    dirty_fraction: f32,
    distribution: DirtyDistribution,
    entropy: DirtyEntropy,
) -> Vec<u8> {
    let mut frame = base.to_vec();
    let tiles_x = w.div_ceil(TILE_SIZE);
    let dirty_tiles = dirty_tile_indices(w, h, dirty_fraction, distribution, 0x4857_434f_4443);
    let mut rng = SplitMix64::new(0x4857_434f_4443_0001);

    for tile_idx in dirty_tiles {
        let tx = tile_idx % tiles_x;
        let ty = tile_idx / tiles_x;
        let x0 = tx * TILE_SIZE;
        let y0 = ty * TILE_SIZE;
        let x1 = (x0 + TILE_SIZE).min(w);
        let y1 = (y0 + TILE_SIZE).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                let off = (y * w + x) * 4;
                match entropy {
                    DirtyEntropy::Invert => {
                        frame[off] = 255 - base[off];
                        frame[off + 1] = 255 - base[off + 1];
                        frame[off + 2] = 255 - base[off + 2];
                    }
                    DirtyEntropy::Noise => {
                        let noise = rng.next_u64();
                        frame[off] = noise as u8;
                        frame[off + 1] = (noise >> 8) as u8;
                        frame[off + 2] = (noise >> 16) as u8;
                    }
                }
            }
        }
    }
    frame
}

fn dirty_tile_indices(
    w: usize,
    h: usize,
    dirty_fraction: f32,
    distribution: DirtyDistribution,
    seed: u64,
) -> Vec<usize> {
    let tiles_x = w.div_ceil(TILE_SIZE);
    let tiles_y = h.div_ceil(TILE_SIZE);
    let total_tiles = tiles_x * tiles_y;
    let dirty_count = ((total_tiles as f32) * dirty_fraction).round() as usize;
    let dirty_count = dirty_count.min(total_tiles);

    match distribution {
        DirtyDistribution::Clustered => (0..dirty_count).collect(),
        DirtyDistribution::Scattered => {
            let mut indices: Vec<usize> = (0..total_tiles).collect();
            let mut rng = SplitMix64::new(seed);
            for i in (1..indices.len()).rev() {
                let j = (rng.next_u64() as usize) % (i + 1);
                indices.swap(i, j);
            }
            indices.truncate(dirty_count);
            indices
        }
    }
}

fn ide_typing_scene(w: usize, h: usize) -> Vec<u8> {
    let mut frame = ide_base_frame(w, h);
    let sidebar_w = (w / 6).min(300);
    let x0 = sidebar_w + 48;
    let y0 = 32 + 12 * 24;
    fill_rect(&mut frame, w, h, x0, y0, 260, 16, [33, 37, 43, 255]);
    fill_rect(&mut frame, w, h, x0, y0 + 6, 180, 4, [224, 108, 117, 255]);
    fill_rect(
        &mut frame,
        w,
        h,
        x0 + 190,
        y0 + 1,
        2,
        18,
        [240, 240, 240, 255],
    );
    frame
}

fn browser_scroll_scene(w: usize, h: usize) -> Vec<u8> {
    let mut frame = browser_base_frame(w, h, 0);
    let scroll_px = 96usize.min(h / 4).max(1);
    let newer = browser_base_frame(w, h, scroll_px);
    let content_top = (h / 10).max(80).min(h);
    let content_bottom = h.saturating_sub(24);
    if content_bottom > content_top + scroll_px {
        for y in content_top..(content_bottom - scroll_px) {
            let src = ((y + scroll_px) * w) * 4;
            let dst = (y * w) * 4;
            frame[dst..dst + w * 4].copy_from_slice(&newer[src..src + w * 4]);
        }
    }
    frame
}

fn terminal_scroll_scene(w: usize, h: usize) -> Vec<u8> {
    let mut frame = terminal_base_frame(w, h);
    let scroll_px = 32usize.min(h / 8).max(1);
    if h > scroll_px {
        for y in 0..(h - scroll_px) {
            let src = (y + scroll_px) * w * 4;
            let dst = y * w * 4;
            frame.copy_within(src..src + w * 4, dst);
        }
    }
    let y0 = h.saturating_sub(scroll_px);
    fill_rect(&mut frame, w, h, 0, y0, w, scroll_px, [18, 18, 18, 255]);
    for row in 0..(scroll_px / 8).max(1) {
        let y = y0 + row * 10 + 4;
        for col in 0..80usize {
            let x = 16 + col * 9;
            if x + 6 >= w || y + 2 >= h {
                break;
            }
            fill_rect(&mut frame, w, h, x, y, 6, 2, [84, 255, 112, 255]);
        }
    }
    frame
}

fn ide_base_frame(w: usize, h: usize) -> Vec<u8> {
    let mut frame = solid_frame(w, h, [33, 37, 43, 255]);
    let sidebar_w = (w / 6).min(300);
    fill_rect(&mut frame, w, h, 0, 0, sidebar_w, h, [26, 29, 35, 255]);
    for i in 0..36usize {
        let y = 32 + i * 24;
        if y + 10 >= h {
            break;
        }
        let indent = (i % 4) * 14;
        fill_rect(
            &mut frame,
            w,
            h,
            sidebar_w + 40 + indent,
            y,
            180 + (i % 7) * 18,
            3,
            [92, 99, 112, 255],
        );
    }
    frame
}

fn terminal_base_frame(w: usize, h: usize) -> Vec<u8> {
    let mut frame = solid_frame(w, h, [18, 18, 18, 255]);
    for row in 0..(h / 18).max(1) {
        let y = 8 + row * 18;
        if y + 4 >= h {
            break;
        }
        for col in 0..96usize {
            let x = 16 + col * 9;
            if x + 6 >= w {
                break;
            }
            let color = match (row + col) % 11 {
                0 => [97, 175, 239, 255],
                1 => [224, 108, 117, 255],
                2 => [152, 195, 121, 255],
                _ => [120, 132, 150, 255],
            };
            fill_rect(&mut frame, w, h, x, y, 6, 3, color);
        }
    }
    frame
}

fn browser_base_frame(w: usize, h: usize, offset: usize) -> Vec<u8> {
    let mut frame = solid_frame(w, h, [245, 247, 250, 255]);
    let toolbar_h = (h / 12).max(56).min(h);
    fill_rect(&mut frame, w, h, 0, 0, w, toolbar_h, [232, 235, 240, 255]);
    fill_rect(
        &mut frame,
        w,
        h,
        72.min(w),
        16.min(h),
        w.saturating_sub(144),
        28.min(h),
        [255, 255, 255, 255],
    );
    let card_w = (w / 3).max(220);
    for i in 0..24usize {
        let y = toolbar_h + 24 + i * 78usize.saturating_sub(offset % 78);
        if y >= h {
            continue;
        }
        let x = 48 + (i % 2) * (card_w + 24);
        fill_rect(
            &mut frame,
            w,
            h,
            x,
            y,
            card_w.min(w.saturating_sub(x)),
            54,
            [255, 255, 255, 255],
        );
        fill_rect(&mut frame, w, h, x + 18, y + 14, 160, 6, [80, 92, 110, 255]);
        fill_rect(
            &mut frame,
            w,
            h,
            x + 18,
            y + 30,
            card_w.saturating_sub(54),
            4,
            [180, 188, 202, 255],
        );
    }
    frame
}

fn fill_rect(
    frame: &mut [u8],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    rw: usize,
    rh: usize,
    bgra: [u8; 4],
) {
    let x1 = (x + rw).min(w);
    let y1 = (y + rh).min(h);
    for py in y.min(h)..y1 {
        for px in x.min(w)..x1 {
            let off = (py * w + px) * 4;
            frame[off..off + 4].copy_from_slice(&bgra);
        }
    }
}

fn default_out_dir() -> PathBuf {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    PathBuf::from("reports")
        .join("hardware-codec-prod")
        .join(seconds.to_string())
}

fn parse_codecs(value: &str) -> Result<Vec<NvencCodec>, String> {
    value
        .split(',')
        .map(|part| match part.trim().to_ascii_lowercase().as_str() {
            "h264" => Ok(NvencCodec::H264),
            "h265" | "hevc" => Ok(NvencCodec::H265),
            "av1" => Ok(NvencCodec::Av1),
            other => Err(format!("unknown codec: {other}")),
        })
        .collect()
}

fn parse_next<T>(args: &mut impl Iterator<Item = String>, name: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    let value = next_arg(args, name)?;
    value
        .parse()
        .map_err(|_| format!("invalid value for {name}: {value}"))
}

fn next_arg(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value after {name}"))
}

fn print_help() {
    eprintln!(
        "Usage: cargo run --release --features live-vp9-mf --bin hardware_codec_prod_bench -- [options]\n\
         \n\
         Options:\n\
           --width N          frame width, default 1920\n\
           --height N         frame height, default 1080\n\
           --fps N            target fps, default 60\n\
           --bitrate N        target bitrate bps, default 8000000\n\
           --codec LIST       comma-separated H264,H265,AV1; default H264,H265\n\
           --iterations N     measured iterations per operation, default 300\n\
           --warmup N         warmup iterations per operation, default 30\n\
           --quick            cap at 40 iterations / 5 warmup\n\
           --out PATH         output directory, default reports/hardware-codec-prod/<unix-seconds>\n"
    );
}
