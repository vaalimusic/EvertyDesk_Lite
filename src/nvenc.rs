#![allow(dead_code)] // Packetizers are retained for the direct NVENC backend and unit tests.

use std::sync::OnceLock;

#[cfg(nvenc_api_ffi)]
use std::ffi::{c_char, c_void, CString};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NvencCodec {
    H264,
    H265,
    Av1,
}

impl NvencCodec {
    pub fn label(self) -> &'static str {
        match self {
            Self::H264 => "H264",
            Self::H265 => "H265",
            Self::Av1 => "AV1",
        }
    }
}

#[derive(Clone, Debug)]
pub struct NvencPacket {
    pub codec: NvencCodec,
    pub bytes: Vec<u8>,
    pub key: bool,
}

pub struct NvencEncoder {
    codec: NvencCodec,
    width: u32,
    height: u32,
    fps: u32,
}

impl NvencEncoder {
    pub fn new(
        codec: NvencCodec,
        width: u32,
        height: u32,
        fps: u32,
        _bitrate: u32,
    ) -> Result<Self, String> {
        let _ = (codec, width, height, fps);
        Err("Direct NVENC encoder backend is not implemented yet".to_owned())
    }

    pub fn matches(&self, codec: NvencCodec, width: u32, height: u32, fps: u32) -> bool {
        self.codec == codec
            && self.width == width.max(2)
            && self.height == height.max(2)
            && self.fps == fps.clamp(5, 60)
    }

    pub fn encode_bgra(&mut self, _bgra: &[u8]) -> Result<Option<NvencPacket>, String> {
        Err("Direct NVENC encoder backend is not implemented yet".to_owned())
    }
}

pub fn nvenc_encoder_codecs() -> Vec<NvencCodec> {
    Vec::new()
}

pub fn nvenc_encoder_names() -> Option<Vec<String>> {
    Some(Vec::new())
}

pub fn nvenc_supported_platform() -> bool {
    cfg!(any(target_os = "linux", windows))
}

pub fn nvencode_api_probe() -> &'static NvencodeApiProbe {
    static PROBE: OnceLock<NvencodeApiProbe> = OnceLock::new();
    PROBE.get_or_init(probe_nvencode_api)
}

pub fn nvencode_api_available() -> bool {
    nvencode_api_probe().function_list_ready
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NvencodeApiProbe {
    pub library: Option<String>,
    pub max_supported_version: Option<String>,
    pub max_supported_raw: Option<u32>,
    pub create_instance_status: Option<i32>,
    pub function_list_ready: bool,
    pub error: Option<String>,
}

impl NvencodeApiProbe {
    pub fn label(&self) -> String {
        if self.function_list_ready {
            let version = self
                .max_supported_version
                .as_deref()
                .unwrap_or("unknown driver API");
            let library = self.library.as_deref().unwrap_or("driver library");
            return format!("NvEncodeAPI ready ({version}, {library})");
        }
        if let Some(error) = &self.error {
            return format!("NvEncodeAPI unavailable: {error}");
        }
        "NvEncodeAPI unavailable".to_owned()
    }
}

pub(crate) struct Packetizer {
    inner: PacketizerInner,
}

enum PacketizerInner {
    AnnexB(AnnexBPacketizer),
    Ivf(IvfPacketizer),
}

impl Packetizer {
    pub(crate) fn new(codec: NvencCodec) -> Self {
        let inner = match codec {
            NvencCodec::H264 | NvencCodec::H265 => {
                PacketizerInner::AnnexB(AnnexBPacketizer::new(codec))
            }
            NvencCodec::Av1 => PacketizerInner::Ivf(IvfPacketizer::new()),
        };
        Self { inner }
    }

    pub(crate) fn push(&mut self, data: &[u8]) -> Vec<NvencPacket> {
        match &mut self.inner {
            PacketizerInner::AnnexB(parser) => parser.push(data),
            PacketizerInner::Ivf(parser) => parser.push(data),
        }
    }

    pub(crate) fn finish(&mut self) -> Vec<NvencPacket> {
        match &mut self.inner {
            PacketizerInner::AnnexB(parser) => parser.finish(),
            PacketizerInner::Ivf(parser) => parser.finish(),
        }
    }
}

struct AnnexBPacketizer {
    codec: NvencCodec,
    buffer: Vec<u8>,
    first_packet: bool,
}

impl AnnexBPacketizer {
    fn new(codec: NvencCodec) -> Self {
        Self {
            codec,
            buffer: Vec::new(),
            first_packet: true,
        }
    }

    fn push(&mut self, data: &[u8]) -> Vec<NvencPacket> {
        self.buffer.extend_from_slice(data);
        let mut packets = Vec::new();
        loop {
            let auds = annex_b_aud_offsets(self.codec, &self.buffer);
            if auds.len() < 2 {
                break;
            }
            let end = auds[1];
            let bytes: Vec<u8> = self.buffer.drain(..end).collect();
            if !annex_b_has_video_payload(self.codec, &bytes) {
                continue;
            }
            let key = self.first_packet || annex_b_is_key(self.codec, &bytes);
            self.first_packet = false;
            packets.push(NvencPacket {
                codec: self.codec,
                bytes,
                key,
            });
        }
        packets
    }

    fn finish(&mut self) -> Vec<NvencPacket> {
        if self.buffer.is_empty() || !annex_b_has_video_payload(self.codec, &self.buffer) {
            self.buffer.clear();
            return Vec::new();
        }
        let bytes = std::mem::take(&mut self.buffer);
        let key = self.first_packet || annex_b_is_key(self.codec, &bytes);
        self.first_packet = false;
        vec![NvencPacket {
            codec: self.codec,
            bytes,
            key,
        }]
    }
}

struct IvfPacketizer {
    buffer: Vec<u8>,
    header_done: bool,
    first_packet: bool,
}

impl IvfPacketizer {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            header_done: false,
            first_packet: true,
        }
    }

    fn push(&mut self, data: &[u8]) -> Vec<NvencPacket> {
        self.buffer.extend_from_slice(data);
        let mut packets = Vec::new();
        if !self.header_done {
            if self.buffer.len() < 32 {
                return packets;
            }
            self.buffer.drain(..32);
            self.header_done = true;
        }
        loop {
            if self.buffer.len() < 12 {
                break;
            }
            let frame_size = u32::from_le_bytes([
                self.buffer[0],
                self.buffer[1],
                self.buffer[2],
                self.buffer[3],
            ]) as usize;
            if self.buffer.len() < 12 + frame_size {
                break;
            }
            self.buffer.drain(..12);
            let bytes: Vec<u8> = self.buffer.drain(..frame_size).collect();
            let key = self.first_packet || av1_payload_has_sequence_header(&bytes);
            self.first_packet = false;
            packets.push(NvencPacket {
                codec: NvencCodec::Av1,
                bytes,
                key,
            });
        }
        packets
    }

    fn finish(&mut self) -> Vec<NvencPacket> {
        Vec::new()
    }
}

fn annex_b_aud_offsets(codec: NvencCodec, bytes: &[u8]) -> Vec<usize> {
    annex_b_nals(bytes)
        .into_iter()
        .filter_map(|(offset, nal)| {
            if annex_b_is_aud(codec, nal) {
                Some(offset)
            } else {
                None
            }
        })
        .collect()
}

fn annex_b_has_video_payload(codec: NvencCodec, bytes: &[u8]) -> bool {
    annex_b_nals(bytes).into_iter().any(|(_, nal)| match codec {
        NvencCodec::H264 => matches!(nal.first().map(|byte| byte & 0x1f), Some(1..=5)),
        NvencCodec::H265 => matches!(nal.first().map(|byte| (byte >> 1) & 0x3f), Some(0..=31)),
        NvencCodec::Av1 => false,
    })
}

fn annex_b_is_key(codec: NvencCodec, bytes: &[u8]) -> bool {
    annex_b_nals(bytes).into_iter().any(|(_, nal)| match codec {
        NvencCodec::H264 => matches!(nal.first().map(|byte| byte & 0x1f), Some(5 | 7)),
        NvencCodec::H265 => matches!(
            nal.first().map(|byte| (byte >> 1) & 0x3f),
            Some(19..=21 | 32 | 33)
        ),
        NvencCodec::Av1 => false,
    })
}

fn annex_b_is_aud(codec: NvencCodec, nal: &[u8]) -> bool {
    match codec {
        NvencCodec::H264 => matches!(nal.first().map(|byte| byte & 0x1f), Some(9)),
        NvencCodec::H265 => matches!(nal.first().map(|byte| (byte >> 1) & 0x3f), Some(35)),
        NvencCodec::Av1 => false,
    }
}

fn annex_b_nals(bytes: &[u8]) -> Vec<(usize, &[u8])> {
    let mut nals = Vec::new();
    let mut pos = 0;
    while let Some((start, start_len)) = find_start_code(bytes, pos) {
        let nal_start = start + start_len;
        let next = find_start_code(bytes, nal_start)
            .map(|(next, _)| next)
            .unwrap_or(bytes.len());
        if nal_start < next {
            nals.push((start, &bytes[nal_start..next]));
        }
        pos = next;
    }
    nals
}

fn find_start_code(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i + 3 <= bytes.len() {
        if bytes[i] == 0 && bytes[i + 1] == 0 {
            if bytes[i + 2] == 1 {
                return Some((i, 3));
            }
            if i + 4 <= bytes.len() && bytes[i + 2] == 0 && bytes[i + 3] == 1 {
                return Some((i, 4));
            }
        }
        i += 1;
    }
    None
}

fn av1_payload_has_sequence_header(bytes: &[u8]) -> bool {
    let mut i = 0;
    while i < bytes.len() {
        let header = bytes[i];
        i += 1;
        let obu_type = (header >> 3) & 0x0f;
        let has_extension = (header & 0x04) != 0;
        let has_size = (header & 0x02) != 0;
        if has_extension {
            i = i.saturating_add(1);
        }
        if !has_size {
            return obu_type == 1;
        }
        let Some((size, used)) = read_leb128(&bytes[i..]) else {
            return obu_type == 1;
        };
        i = i.saturating_add(used);
        if obu_type == 1 {
            return true;
        }
        i = i.saturating_add(size);
    }
    false
}

fn read_leb128(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    for (i, byte) in bytes.iter().take(8).enumerate() {
        value |= usize::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

#[cfg(nvenc_api_ffi)]
fn probe_nvencode_api() -> NvencodeApiProbe {
    let mut last_error = None;
    for name in nvencode_library_names() {
        match DynamicLibrary::open(name) {
            Ok(library) => return probe_nvencode_api_from_library(library, name),
            Err(err) => last_error = Some(err),
        }
    }
    NvencodeApiProbe {
        error: Some(last_error.unwrap_or_else(|| "driver library not found".to_owned())),
        ..Default::default()
    }
}

#[cfg(not(nvenc_api_ffi))]
fn probe_nvencode_api() -> NvencodeApiProbe {
    if !nvenc_supported_platform() {
        return NvencodeApiProbe {
            error: Some("NVENC is not supported on macOS".to_owned()),
            ..Default::default()
        };
    }
    NvencodeApiProbe {
        error: Some("built without live-nvenc-sdk feature/SDK".to_owned()),
        ..Default::default()
    }
}

#[cfg(nvenc_api_ffi)]
fn probe_nvencode_api_from_library(library: DynamicLibrary, name: &str) -> NvencodeApiProbe {
    let mut probe = NvencodeApiProbe {
        library: Some(name.to_owned()),
        ..Default::default()
    };

    unsafe {
        if let Ok(get_max_version) = library
            .symbol::<NvEncodeApiGetMaxSupportedVersion>(b"NvEncodeAPIGetMaxSupportedVersion\0")
        {
            let mut raw = 0_u32;
            let status = get_max_version(&mut raw);
            if status == 0 {
                probe.max_supported_raw = Some(raw);
                probe.max_supported_version = Some(format_nvenc_api_version(raw));
            }
        }

        let create_instance =
            match library.symbol::<NvEncodeApiCreateInstance>(b"NvEncodeAPICreateInstance\0") {
                Ok(create_instance) => create_instance,
                Err(err) => {
                    probe.error = Some(err);
                    return probe;
                }
            };

        let mut function_list = NvEncodeApiFunctionList::new();
        let status = create_instance(&mut function_list);
        probe.create_instance_status = Some(status);
        probe.function_list_ready =
            status == 0 && function_list.ptrs.iter().any(|ptr| !ptr.is_null());
        if !probe.function_list_ready {
            probe.error = Some(format!("NvEncodeAPICreateInstance status {status}"));
        }
    }

    probe
}

#[cfg(nvenc_api_ffi)]
#[repr(C)]
struct NvEncodeApiFunctionList {
    version: u32,
    reserved: u32,
    ptrs: [*mut c_void; 318],
}

#[cfg(nvenc_api_ffi)]
impl NvEncodeApiFunctionList {
    fn new() -> Self {
        Self {
            version: nvencapi_struct_version(2),
            reserved: 0,
            ptrs: [std::ptr::null_mut(); 318],
        }
    }
}

#[cfg(nvenc_api_ffi)]
type NvEncodeApiCreateInstance = unsafe extern "system" fn(*mut NvEncodeApiFunctionList) -> i32;

#[cfg(all(nvenc_api_ffi, not(windows)))]
type NvEncodeApiGetMaxSupportedVersion = unsafe extern "C" fn(*mut u32) -> i32;

#[cfg(all(nvenc_api_ffi, windows))]
type NvEncodeApiGetMaxSupportedVersion = unsafe extern "system" fn(*mut u32) -> i32;

#[cfg(nvenc_api_ffi)]
const fn nvencapi_struct_version(version: u32) -> u32 {
    (13 | (0 << 24)) | (version << 16) | (0x7 << 28)
}

#[cfg(nvenc_api_ffi)]
fn format_nvenc_api_version(raw: u32) -> String {
    let major = raw & 0x00ff_ffff;
    let minor = raw >> 24;
    format!("{major}.{minor}")
}

#[cfg(all(nvenc_api_ffi, target_pointer_width = "64", windows))]
fn nvencode_library_names() -> &'static [&'static str] {
    &["nvEncodeAPI64.dll", "nvEncodeAPI.dll"]
}

#[cfg(all(nvenc_api_ffi, target_pointer_width = "32", windows))]
fn nvencode_library_names() -> &'static [&'static str] {
    &["nvEncodeAPI.dll"]
}

#[cfg(all(nvenc_api_ffi, not(windows)))]
fn nvencode_library_names() -> &'static [&'static str] {
    &["libnvidia-encode.so.1", "libnvidia-encode.so"]
}

#[cfg(nvenc_api_ffi)]
struct DynamicLibrary {
    handle: *mut c_void,
}

#[cfg(nvenc_api_ffi)]
impl DynamicLibrary {
    fn open(name: &str) -> Result<Self, String> {
        let name = CString::new(name).map_err(|_| "library name contains NUL".to_owned())?;
        #[cfg(windows)]
        unsafe {
            let handle = LoadLibraryA(name.as_ptr());
            if handle.is_null() {
                Err("LoadLibraryA failed".to_owned())
            } else {
                Ok(Self { handle })
            }
        }
        #[cfg(not(windows))]
        unsafe {
            let handle = libc::dlopen(name.as_ptr(), libc::RTLD_NOW);
            if handle.is_null() {
                Err(dl_error())
            } else {
                Ok(Self { handle })
            }
        }
    }

    unsafe fn symbol<T: Copy>(&self, name: &[u8]) -> Result<T, String> {
        #[cfg(windows)]
        {
            let ptr = GetProcAddress(self.handle, name.as_ptr() as *const c_char);
            if ptr.is_null() {
                Err("GetProcAddress failed".to_owned())
            } else {
                Ok(std::mem::transmute_copy(&ptr))
            }
        }
        #[cfg(not(windows))]
        {
            let ptr = libc::dlsym(self.handle, name.as_ptr() as *const c_char);
            if ptr.is_null() {
                Err(dl_error())
            } else {
                Ok(std::mem::transmute_copy(&ptr))
            }
        }
    }
}

#[cfg(nvenc_api_ffi)]
impl Drop for DynamicLibrary {
    fn drop(&mut self) {
        unsafe {
            #[cfg(windows)]
            {
                let _ = FreeLibrary(self.handle);
            }
            #[cfg(not(windows))]
            {
                let _ = libc::dlclose(self.handle);
            }
        }
    }
}

#[cfg(all(nvenc_api_ffi, not(windows)))]
fn dl_error() -> String {
    unsafe {
        let err = libc::dlerror();
        if err.is_null() {
            "dlopen/dlsym failed".to_owned()
        } else {
            std::ffi::CStr::from_ptr(err).to_string_lossy().into_owned()
        }
    }
}

#[cfg(all(nvenc_api_ffi, windows))]
#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryA(name: *const c_char) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annex_b_packetizer_splits_h264_on_aud() {
        let mut parser = AnnexBPacketizer::new(NvencCodec::H264);
        let data = [
            0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 7, 1, 2, 0, 0, 0, 1, 5, 3, 4, 0, 0, 0, 1, 9, 0xf0, 0,
            0, 0, 1, 1, 5, 6,
        ];

        let packets = parser.push(&data);

        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].codec, NvencCodec::H264);
        assert!(packets[0].key);
        assert!(packets[0].bytes.ends_with(&[5, 3, 4]));
    }

    #[test]
    fn ivf_packetizer_strips_container_headers() {
        let mut parser = IvfPacketizer::new();
        let mut data = vec![0_u8; 32];
        data.extend_from_slice(&3_u32.to_le_bytes());
        data.extend_from_slice(&0_u64.to_le_bytes());
        data.extend_from_slice(&[0x0a, 0x01, 0x00]);

        let packets = parser.push(&data);

        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].codec, NvencCodec::Av1);
        assert_eq!(packets[0].bytes, vec![0x0a, 0x01, 0x00]);
    }
}
