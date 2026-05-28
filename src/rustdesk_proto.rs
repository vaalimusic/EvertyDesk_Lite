use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct PunchHoleRequest {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(enumeration = "NatType", tag = "2")]
    pub nat_type: i32,
    #[prost(string, tag = "3")]
    pub licence_key: String,
    #[prost(enumeration = "ConnType", tag = "4")]
    pub conn_type: i32,
    #[prost(string, tag = "5")]
    pub token: String,
    #[prost(string, tag = "6")]
    pub version: String,
    #[prost(int32, tag = "7")]
    pub udp_port: i32,
    #[prost(bool, tag = "8")]
    pub force_relay: bool,
    #[prost(int32, tag = "9")]
    pub upnp_port: i32,
    #[prost(bytes, tag = "10")]
    pub socket_addr_v6: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PunchHoleResponse {
    #[prost(bytes, tag = "1")]
    pub socket_addr: Vec<u8>,
    #[prost(bytes, tag = "2")]
    pub pk: Vec<u8>,
    #[prost(enumeration = "PunchHoleFailure", tag = "3")]
    pub failure: i32,
    #[prost(string, tag = "4")]
    pub relay_server: String,
    #[prost(string, tag = "7")]
    pub other_failure: String,
    #[prost(int32, tag = "9")]
    pub feedback: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct RelayResponse {
    #[prost(bytes, tag = "1")]
    pub socket_addr: Vec<u8>,
    #[prost(string, tag = "3")]
    pub relay_server: String,
    #[prost(string, tag = "6")]
    pub refuse_reason: String,
    #[prost(string, tag = "7")]
    pub version: String,
    #[prost(int32, tag = "9")]
    pub feedback: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct RequestRelay {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, tag = "2")]
    pub uuid: String,
    #[prost(bytes, tag = "3")]
    pub socket_addr: Vec<u8>,
    #[prost(string, tag = "4")]
    pub relay_server: String,
    #[prost(bool, tag = "5")]
    pub secure: bool,
    #[prost(string, tag = "6")]
    pub licence_key: String,
    #[prost(enumeration = "ConnType", tag = "7")]
    pub conn_type: i32,
    #[prost(string, tag = "8")]
    pub token: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct OnlineRequest {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, repeated, tag = "2")]
    pub peers: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct OnlineResponse {
    #[prost(bytes, tag = "1")]
    pub states: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct RendezvousMessage {
    #[prost(oneof = "rendezvous_message::Union", tags = "8, 11, 18, 19, 23, 24")]
    pub union: Option<rendezvous_message::Union>,
}

pub mod rendezvous_message {
    use prost::Oneof;

    use super::{
        OnlineRequest, OnlineResponse, PunchHoleRequest, PunchHoleResponse, RelayResponse,
        RequestRelay,
    };

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Union {
        #[prost(message, tag = "8")]
        PunchHoleRequest(PunchHoleRequest),
        #[prost(message, tag = "11")]
        PunchHoleResponse(PunchHoleResponse),
        #[prost(message, tag = "18")]
        RequestRelay(RequestRelay),
        #[prost(message, tag = "19")]
        RelayResponse(RelayResponse),
        #[prost(message, tag = "23")]
        OnlineRequest(OnlineRequest),
        #[prost(message, tag = "24")]
        OnlineResponse(OnlineResponse),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum ConnType {
    DefaultConn = 0,
    FileTransfer = 1,
    PortForward = 2,
    Rdp = 3,
    ViewCamera = 4,
    Terminal = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum NatType {
    UnknownNat = 0,
    Asymmetric = 1,
    Symmetric = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum PunchHoleFailure {
    IdNotExist = 0,
    Offline = 2,
    LicenseMismatch = 3,
    LicenseOveruse = 4,
}

pub fn encode_message(message: &RendezvousMessage) -> Vec<u8> {
    message.encode_to_vec()
}

pub fn decode_message(bytes: &[u8]) -> Result<RendezvousMessage, prost::DecodeError> {
    RendezvousMessage::decode(bytes)
}

#[derive(Clone, PartialEq, Message)]
pub struct LoginRequest {
    #[prost(string, tag = "1")]
    pub username: String,
    #[prost(bytes, tag = "2")]
    pub password: Vec<u8>,
    #[prost(string, tag = "4")]
    pub my_id: String,
    #[prost(string, tag = "5")]
    pub my_name: String,
    #[prost(message, optional, tag = "6")]
    pub option: Option<OptionMessage>,
    #[prost(bool, tag = "9")]
    pub video_ack_required: bool,
    #[prost(string, tag = "11")]
    pub version: String,
    #[prost(string, tag = "13")]
    pub my_platform: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct OptionMessage {
    #[prost(message, optional, tag = "10")]
    pub supported_decoding: Option<SupportedDecoding>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SupportedDecoding {
    #[prost(int32, tag = "1")]
    pub ability_vp9: i32,
    #[prost(int32, tag = "2")]
    pub ability_h264: i32,
    #[prost(int32, tag = "3")]
    pub ability_h265: i32,
    #[prost(enumeration = "PreferCodec", tag = "4")]
    pub prefer: i32,
    #[prost(int32, tag = "5")]
    pub ability_vp8: i32,
    #[prost(int32, tag = "6")]
    pub ability_av1: i32,
    #[prost(message, optional, tag = "7")]
    pub i444: Option<CodecAbility>,
    #[prost(enumeration = "Chroma", tag = "8")]
    pub prefer_chroma: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct CodecAbility {
    #[prost(bool, tag = "1")]
    pub vp8: bool,
    #[prost(bool, tag = "2")]
    pub vp9: bool,
    #[prost(bool, tag = "3")]
    pub av1: bool,
    #[prost(bool, tag = "4")]
    pub h264: bool,
    #[prost(bool, tag = "5")]
    pub h265: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
pub enum PreferCodec {
    Auto = 0,
    Vp9 = 1,
    H264 = 2,
    H265 = 3,
    Vp8 = 4,
    Av1 = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
pub enum Chroma {
    I420 = 0,
    I444 = 1,
}

#[derive(Clone, PartialEq, Message)]
pub struct Hash {
    #[prost(string, tag = "1")]
    pub salt: String,
    #[prost(string, tag = "2")]
    pub challenge: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct SignedId {
    #[prost(bytes, tag = "1")]
    pub id: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PublicKey {
    #[prost(bytes, tag = "1")]
    pub asymmetric_value: Vec<u8>,
    #[prost(bytes, tag = "2")]
    pub symmetric_value: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PeerInfo {
    #[prost(string, tag = "1")]
    pub username: String,
    #[prost(string, tag = "2")]
    pub hostname: String,
    #[prost(string, tag = "3")]
    pub platform: String,
    #[prost(message, repeated, tag = "4")]
    pub displays: Vec<DisplayInfo>,
    #[prost(int32, tag = "5")]
    pub current_display: i32,
    #[prost(string, tag = "7")]
    pub version: String,
    #[prost(message, optional, tag = "13")]
    pub windows_sessions: Option<WindowsSessions>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WindowsSession {
    #[prost(uint32, tag = "1")]
    pub sid: u32,
    #[prost(string, tag = "2")]
    pub name: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct WindowsSessions {
    #[prost(message, repeated, tag = "1")]
    pub sessions: Vec<WindowsSession>,
    #[prost(uint32, tag = "2")]
    pub current_sid: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct DisplayInfo {
    #[prost(sint32, tag = "1")]
    pub x: i32,
    #[prost(sint32, tag = "2")]
    pub y: i32,
    #[prost(int32, tag = "3")]
    pub width: i32,
    #[prost(int32, tag = "4")]
    pub height: i32,
    #[prost(string, tag = "5")]
    pub name: String,
    #[prost(bool, tag = "6")]
    pub online: bool,
    #[prost(bool, tag = "7")]
    pub cursor_embedded: bool,
    #[prost(double, tag = "9")]
    pub scale: f64,
}

#[derive(Clone, PartialEq, Message)]
pub struct LoginResponse {
    #[prost(oneof = "login_response::Union", tags = "1, 2")]
    pub union: Option<login_response::Union>,
}

pub mod login_response {
    use prost::Oneof;

    use super::PeerInfo;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Union {
        #[prost(string, tag = "1")]
        Error(String),
        #[prost(message, tag = "2")]
        PeerInfo(PeerInfo),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct PeerMessage {
    #[prost(
        oneof = "peer_message::Union",
        tags = "3, 4, 5, 6, 7, 8, 9, 10, 15, 19, 25, 29, 30"
    )]
    pub union: Option<peer_message::Union>,
}

pub mod peer_message {
    use prost::Oneof;

    use super::{
        Hash, KeyEvent, LoginRequest, LoginResponse, Misc, MouseEvent, PeerInfo, PublicKey,
        ScreenshotRequest, ScreenshotResponse, SignedId, TestDelay, VideoFrame,
    };

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Union {
        #[prost(message, tag = "3")]
        SignedId(SignedId),
        #[prost(message, tag = "4")]
        PublicKey(PublicKey),
        #[prost(message, tag = "5")]
        TestDelay(TestDelay),
        #[prost(message, tag = "6")]
        VideoFrame(VideoFrame),
        #[prost(message, tag = "7")]
        LoginRequest(LoginRequest),
        #[prost(message, tag = "8")]
        LoginResponse(LoginResponse),
        #[prost(message, tag = "9")]
        Hash(Hash),
        #[prost(message, tag = "10")]
        MouseEvent(MouseEvent),
        #[prost(message, tag = "15")]
        KeyEvent(KeyEvent),
        #[prost(message, tag = "19")]
        Misc(Misc),
        #[prost(message, tag = "25")]
        PeerInfo(PeerInfo),
        #[prost(message, tag = "29")]
        ScreenshotRequest(ScreenshotRequest),
        #[prost(message, tag = "30")]
        ScreenshotResponse(ScreenshotResponse),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct MouseEvent {
    #[prost(int32, tag = "1")]
    pub mask: i32,
    #[prost(sint32, tag = "2")]
    pub x: i32,
    #[prost(sint32, tag = "3")]
    pub y: i32,
    #[prost(enumeration = "ControlKey", repeated, tag = "4")]
    pub modifiers: Vec<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct KeyEvent {
    #[prost(bool, tag = "1")]
    pub down: bool,
    #[prost(bool, tag = "2")]
    pub press: bool,
    #[prost(oneof = "key_event::Union", tags = "3, 5")]
    pub union: Option<key_event::Union>,
    #[prost(enumeration = "ControlKey", repeated, tag = "8")]
    pub modifiers: Vec<i32>,
    #[prost(enumeration = "KeyboardMode", tag = "9")]
    pub mode: i32,
}

pub mod key_event {
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Union {
        #[prost(enumeration = "super::ControlKey", tag = "3")]
        ControlKey(i32),
        #[prost(uint32, tag = "5")]
        Unicode(u32),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
pub enum KeyboardMode {
    Legacy = 0,
    Map = 1,
    Translate = 2,
    Auto = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
pub enum ControlKey {
    Unknown = 0,
    Alt = 1,
    Backspace = 2,
    CapsLock = 3,
    Control = 4,
    Delete = 5,
    DownArrow = 6,
    End = 7,
    Escape = 8,
    F1 = 9,
    F10 = 10,
    F11 = 11,
    F12 = 12,
    F2 = 13,
    F3 = 14,
    F4 = 15,
    F5 = 16,
    F6 = 17,
    F7 = 18,
    F8 = 19,
    F9 = 20,
    Home = 21,
    LeftArrow = 22,
    Meta = 23,
    PageDown = 25,
    PageUp = 26,
    Return = 27,
    RightArrow = 28,
    Shift = 29,
    Space = 30,
    Tab = 31,
    UpArrow = 32,
    Insert = 58,
    NumpadEnter = 72,
    CtrlAltDel = 100,
    LockScreen = 101,
}

#[derive(Clone, PartialEq, Message)]
pub struct ScreenshotRequest {
    #[prost(int32, tag = "1")]
    pub display: i32,
    #[prost(string, tag = "2")]
    pub sid: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ScreenshotResponse {
    #[prost(string, tag = "1")]
    pub sid: String,
    #[prost(string, tag = "2")]
    pub msg: String,
    #[prost(bytes, tag = "3")]
    pub data: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TestDelay {
    #[prost(int64, tag = "1")]
    pub time: i64,
    #[prost(bool, tag = "2")]
    pub from_client: bool,
    #[prost(uint32, tag = "3")]
    pub last_delay: u32,
    #[prost(uint32, tag = "4")]
    pub target_bitrate: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct Misc {
    #[prost(oneof = "misc::Union", tags = "5, 10, 12, 31, 35")]
    pub union: Option<misc::Union>,
}

pub mod misc {
    use prost::Oneof;

    use super::SwitchDisplay;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Union {
        #[prost(message, tag = "5")]
        SwitchDisplay(SwitchDisplay),
        #[prost(bool, tag = "10")]
        RefreshVideo(bool),
        #[prost(bool, tag = "12")]
        VideoReceived(bool),
        #[prost(int32, tag = "31")]
        RefreshVideoDisplay(i32),
        #[prost(uint32, tag = "35")]
        SelectedSid(u32),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct SwitchDisplay {
    #[prost(int32, tag = "1")]
    pub display: i32,
    #[prost(sint32, tag = "2")]
    pub x: i32,
    #[prost(sint32, tag = "3")]
    pub y: i32,
    #[prost(int32, tag = "4")]
    pub width: i32,
    #[prost(int32, tag = "5")]
    pub height: i32,
    #[prost(bool, tag = "6")]
    pub cursor_embedded: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct EncodedVideoFrame {
    #[prost(bytes, tag = "1")]
    pub data: Vec<u8>,
    #[prost(bool, tag = "2")]
    pub key: bool,
    #[prost(int64, tag = "3")]
    pub pts: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct EncodedVideoFrames {
    #[prost(message, repeated, tag = "1")]
    pub frames: Vec<EncodedVideoFrame>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Rgb {
    #[prost(bool, tag = "1")]
    pub compress: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct Yuv {
    #[prost(bool, tag = "1")]
    pub compress: bool,
    #[prost(int32, tag = "2")]
    pub stride: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct VideoFrame {
    #[prost(oneof = "video_frame::Union", tags = "6, 7, 8, 10, 11, 12, 13")]
    pub union: Option<video_frame::Union>,
    #[prost(int32, tag = "14")]
    pub display: i32,
}

pub mod video_frame {
    use prost::Oneof;

    use super::{EncodedVideoFrames, Rgb, Yuv};

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Union {
        #[prost(message, tag = "6")]
        Vp9s(EncodedVideoFrames),
        #[prost(message, tag = "7")]
        Rgb(Rgb),
        #[prost(message, tag = "8")]
        Yuv(Yuv),
        #[prost(message, tag = "10")]
        H264s(EncodedVideoFrames),
        #[prost(message, tag = "11")]
        H265s(EncodedVideoFrames),
        #[prost(message, tag = "12")]
        Vp8s(EncodedVideoFrames),
        #[prost(message, tag = "13")]
        Av1s(EncodedVideoFrames),
    }
}

pub fn encode_peer_message(message: &PeerMessage) -> Vec<u8> {
    message.encode_to_vec()
}

pub fn decode_peer_message(bytes: &[u8]) -> Result<PeerMessage, prost::DecodeError> {
    PeerMessage::decode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punch_hole_request_round_trips() {
        let message = RendezvousMessage {
            union: Some(rendezvous_message::Union::PunchHoleRequest(
                PunchHoleRequest {
                    id: "123456789".to_owned(),
                    nat_type: NatType::UnknownNat as i32,
                    licence_key: "key".to_owned(),
                    conn_type: ConnType::DefaultConn as i32,
                    token: String::new(),
                    version: "1.4.6".to_owned(),
                    udp_port: 0,
                    force_relay: true,
                    upnp_port: 0,
                    socket_addr_v6: Vec::new(),
                },
            )),
        };

        let encoded = encode_message(&message);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(
            decoded.union,
            Some(rendezvous_message::Union::PunchHoleRequest(_))
        ));
    }

    #[test]
    fn request_relay_round_trips() {
        let message = RendezvousMessage {
            union: Some(rendezvous_message::Union::RequestRelay(RequestRelay {
                id: "123456789".to_owned(),
                uuid: "uuid".to_owned(),
                socket_addr: Vec::new(),
                relay_server: "edesk.server1.everty.ru".to_owned(),
                secure: false,
                licence_key: "key".to_owned(),
                conn_type: ConnType::DefaultConn as i32,
                token: String::new(),
            })),
        };

        let encoded = encode_message(&message);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(
            decoded.union,
            Some(rendezvous_message::Union::RequestRelay(_))
        ));
    }
}
