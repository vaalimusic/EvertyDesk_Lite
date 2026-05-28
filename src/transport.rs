use std::{
    io::{Read, Write},
    net::TcpStream,
    net::ToSocketAddrs,
    sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError},
    thread,
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
#[cfg(feature = "live-h264")]
use openh264::formats::YUVSource;
use sha2::{Digest, Sha256};

use crate::{
    rustdesk_proto::{
        decode_message, decode_peer_message, encode_message, encode_peer_message, misc,
        peer_message, rendezvous_message, video_frame, Chroma, CodecAbility, ConnType, ControlKey,
        EncodedVideoFrames, KeyEvent, KeyboardMode, LoginRequest, Misc, MouseEvent, NatType,
        OnlineRequest, OptionMessage, PeerMessage, PreferCodec, PublicKey, PunchHoleFailure,
        PunchHoleRequest, RendezvousMessage, RequestRelay, ScreenshotRequest, SupportedDecoding,
        SwitchDisplay,
    },
    settings::ServerConfig,
};

const RENDEZVOUS_PORT: u16 = 21116;
const ONLINE_PORT: u16 = RENDEZVOUS_PORT - 1;
const RELAY_PORT: u16 = 21117;
const SESSION_TICK_MS: u64 = 50;

#[derive(Clone, Debug)]
pub struct ConnectionRequest {
    pub remote_id: String,
    pub password: String,
    pub server: ServerConfig,
}

#[derive(Clone, Debug)]
pub enum ConnectionState {
    Idle,
    RelayReady { remote_id: String },
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum SessionEvent {
    Progress(u8, String),
    Connected(String),
    Frame {
        sid: String,
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
    ScreenshotStats {
        received: u64,
        pending: bool,
    },
    Displays(Vec<RemoteDisplay>),
    Info(String),
    Failed(String),
    Closed,
}

#[derive(Clone, Debug)]
pub struct RemoteDisplay {
    pub index: i32,
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
    pub cursor_embedded: bool,
}

#[derive(Clone, Debug)]
pub enum SessionCommand {
    MouseMove {
        x: i32,
        y: i32,
    },
    MouseDown {
        x: i32,
        y: i32,
    },
    MouseUp {
        x: i32,
        y: i32,
    },
    MouseRightDown {
        x: i32,
        y: i32,
    },
    MouseRightUp {
        x: i32,
        y: i32,
    },
    MouseMiddleDown {
        x: i32,
        y: i32,
    },
    MouseMiddleUp {
        x: i32,
        y: i32,
    },
    MouseWheel {
        x: i32,
        y: i32,
    },
    KeyText(String),
    KeyControl(ControlKey),
    KeyTextWithModifiers {
        text: String,
        modifiers: Vec<ControlKey>,
    },
    KeyControlWithModifiers {
        key: ControlKey,
        modifiers: Vec<ControlKey>,
    },
    KeyEnter,
    Screenshot,
    SetDisplay(RemoteDisplay),
    SetAutoRefresh {
        enabled: bool,
        millis: u64,
    },
    Close,
}

enum DecoderInput {
    Png {
        sid: String,
        png: Vec<u8>,
    },
    H264 {
        sid: String,
        frames: EncodedVideoFrames,
    },
}

impl ConnectionState {
    pub fn as_text(&self) -> String {
        match self {
            Self::Idle => "idle".to_owned(),
            Self::RelayReady { remote_id } => {
                format!("relay session bootstrap complete for {remote_id}")
            }
            Self::Failed(err) => format!("error: {err}"),
        }
    }
}

pub struct TransportClient;

impl TransportClient {
    pub fn check_id_server(server: &ServerConfig) -> Result<(), String> {
        connect_tcp(&server.id_server, RENDEZVOUS_PORT).map(|_| ())
    }

    pub fn query_peer_online(
        server: &ServerConfig,
        local_id: &str,
        remote_id: &str,
    ) -> Result<bool, String> {
        let mut socket = connect_tcp(&server.id_server, ONLINE_PORT)?;
        socket
            .set_read_timeout(Some(Duration::from_secs(4)))
            .map_err(|err| format!("Failed to set online read timeout: {err}"))?;
        let request = RendezvousMessage {
            union: Some(rendezvous_message::Union::OnlineRequest(OnlineRequest {
                id: local_id.to_owned(),
                peers: vec![remote_id.to_owned()],
            })),
        };
        send_framed(&mut socket, &encode_message(&request))?;
        let response = decode_message(&read_framed(&mut socket)?)
            .map_err(|err| format!("Online response decode failed: {err}"))?;
        match response.union {
            Some(rendezvous_message::Union::OnlineResponse(response)) => Ok(response
                .states
                .first()
                .is_some_and(|byte| byte & 0x80 == 0x80)),
            _ => Err("Unexpected online response".to_owned()),
        }
    }

    pub fn connect_with_progress(
        request: ConnectionRequest,
        mut progress: impl FnMut(u8, String),
    ) -> Result<ConnectionState, String> {
        let (_relay_stream, peer_stage, _displays) =
            establish_session(request.clone(), &mut progress)?;

        progress(99, format!("Login stage: {peer_stage}"));
        Ok(ConnectionState::RelayReady {
            remote_id: request.remote_id,
        })
    }

    pub fn run_session(
        request: ConnectionRequest,
        commands: Receiver<SessionCommand>,
        events: Sender<SessionEvent>,
    ) {
        let mut emit_progress = |pct, message: String| {
            let _ = events.send(SessionEvent::Progress(pct, message));
        };

        let (mut relay, peer_stage, displays) = match establish_session(request, &mut emit_progress)
        {
            Ok(session) => session,
            Err(err) => {
                let _ = events.send(SessionEvent::Failed(err));
                return;
            }
        };

        let _ = events.send(SessionEvent::Connected(peer_stage));
        if !displays.is_empty() {
            let _ = events.send(SessionEvent::Displays(displays.clone()));
        }
        let (frame_tx, frame_rx) = mpsc::sync_channel::<DecoderInput>(1);
        let frame_events = events.clone();
        thread::spawn(move || decode_frame_loop(frame_rx, frame_events));

        let _ = relay.set_read_timeout(Some(Duration::from_millis(SESSION_TICK_MS)));
        let mut screenshot_id = 0_u64;
        let mut idle_ticks = 0_u32;
        let mut current_display = 0_i32;
        let mut auto_refresh = true;
        let mut auto_refresh_ticks = 2_u32;
        let mut screenshot_pending = false;
        let mut screenshots_received = 0_u64;
        request_screenshot_if_idle(
            &mut relay,
            &mut screenshot_id,
            current_display,
            &mut screenshot_pending,
            &events,
            screenshots_received,
        );

        loop {
            while let Ok(command) = commands.try_recv() {
                match command {
                    SessionCommand::MouseMove { x, y } => {
                        let _ = send_mouse(&mut relay, MOUSE_TYPE_MOVE, x, y);
                    }
                    SessionCommand::MouseDown { x, y } => {
                        let _ =
                            send_mouse(&mut relay, MOUSE_BUTTON_LEFT << 3 | MOUSE_TYPE_DOWN, x, y);
                    }
                    SessionCommand::MouseUp { x, y } => {
                        let _ =
                            send_mouse(&mut relay, MOUSE_BUTTON_LEFT << 3 | MOUSE_TYPE_UP, x, y);
                    }
                    SessionCommand::MouseRightDown { x, y } => {
                        let _ =
                            send_mouse(&mut relay, MOUSE_BUTTON_RIGHT << 3 | MOUSE_TYPE_DOWN, x, y);
                    }
                    SessionCommand::MouseRightUp { x, y } => {
                        let _ =
                            send_mouse(&mut relay, MOUSE_BUTTON_RIGHT << 3 | MOUSE_TYPE_UP, x, y);
                    }
                    SessionCommand::MouseMiddleDown { x, y } => {
                        let _ =
                            send_mouse(&mut relay, MOUSE_BUTTON_WHEEL << 3 | MOUSE_TYPE_DOWN, x, y);
                    }
                    SessionCommand::MouseMiddleUp { x, y } => {
                        let _ =
                            send_mouse(&mut relay, MOUSE_BUTTON_WHEEL << 3 | MOUSE_TYPE_UP, x, y);
                    }
                    SessionCommand::MouseWheel { x, y } => {
                        let _ = send_mouse(&mut relay, MOUSE_TYPE_WHEEL, x, y);
                    }
                    SessionCommand::KeyText(text) => {
                        let _ = send_text(&mut relay, &text);
                    }
                    SessionCommand::KeyControl(key) => {
                        let _ = send_control_key(&mut relay, key);
                    }
                    SessionCommand::KeyTextWithModifiers { text, modifiers } => {
                        let _ = send_text_with_modifiers(&mut relay, &text, &modifiers);
                    }
                    SessionCommand::KeyControlWithModifiers { key, modifiers } => {
                        let _ = send_control_key_with_modifiers(&mut relay, key, &modifiers);
                    }
                    SessionCommand::KeyEnter => {
                        let _ = send_control_key(&mut relay, ControlKey::Return);
                    }
                    SessionCommand::Screenshot => {
                        request_screenshot_if_idle(
                            &mut relay,
                            &mut screenshot_id,
                            current_display,
                            &mut screenshot_pending,
                            &events,
                            screenshots_received,
                        );
                    }
                    SessionCommand::SetDisplay(display) => {
                        current_display = display.index.max(0);
                        let _ = send_switch_display(&mut relay, current_display, Some(&display));
                        screenshot_pending = false;
                        request_screenshot_if_idle(
                            &mut relay,
                            &mut screenshot_id,
                            current_display,
                            &mut screenshot_pending,
                            &events,
                            screenshots_received,
                        );
                    }
                    SessionCommand::SetAutoRefresh { enabled, millis } => {
                        auto_refresh = enabled;
                        auto_refresh_ticks = ((millis.max(SESSION_TICK_MS) + SESSION_TICK_MS - 1)
                            / SESSION_TICK_MS) as u32;
                    }
                    SessionCommand::Close => {
                        let _ = events.send(SessionEvent::Closed);
                        return;
                    }
                }
            }

            match read_framed(&mut relay) {
                Ok(payload) => match decode_peer_message(&payload) {
                    Ok(message) => {
                        if handle_session_message(message, &mut relay, &events, &frame_tx) {
                            screenshot_pending = false;
                            screenshots_received += 1;
                            idle_ticks = 0;
                            let _ = events.send(SessionEvent::ScreenshotStats {
                                received: screenshots_received,
                                pending: screenshot_pending,
                            });
                            if auto_refresh {
                                request_screenshot_if_idle(
                                    &mut relay,
                                    &mut screenshot_id,
                                    current_display,
                                    &mut screenshot_pending,
                                    &events,
                                    screenshots_received,
                                );
                            }
                        }
                    }
                    Err(err) => {
                        let _ =
                            events.send(SessionEvent::Info(format!("Peer decode skipped: {err}")));
                    }
                },
                Err(err) if is_timeout_error(&err) => {
                    idle_ticks += 1;
                    if auto_refresh && idle_ticks >= auto_refresh_ticks.max(1) {
                        idle_ticks = 0;
                        request_screenshot_if_idle(
                            &mut relay,
                            &mut screenshot_id,
                            current_display,
                            &mut screenshot_pending,
                            &events,
                            screenshots_received,
                        );
                    }
                }
                Err(err) => {
                    let _ = events.send(SessionEvent::Failed(err));
                    return;
                }
            }
        }
    }
}

const MOUSE_TYPE_MOVE: i32 = 0;
const MOUSE_TYPE_DOWN: i32 = 1;
const MOUSE_TYPE_UP: i32 = 2;
const MOUSE_TYPE_WHEEL: i32 = 3;
const MOUSE_BUTTON_LEFT: i32 = 1;
const MOUSE_BUTTON_RIGHT: i32 = 2;
const MOUSE_BUTTON_WHEEL: i32 = 4;

fn establish_session(
    request: ConnectionRequest,
    progress: &mut impl FnMut(u8, String),
) -> Result<(TcpStream, String, Vec<RemoteDisplay>), String> {
    progress(5, "Validating input".to_owned());
    if request.remote_id.is_empty() {
        return Err("Enter remote ID".to_owned());
    }
    if false && request.password.is_empty() {
        return Err("Enter remote password".to_owned());
    }

    progress(15, "Validating server public key".to_owned());
    validate_public_key(&request.server.public_key)?;

    progress(30, "Connecting to ID server".to_owned());
    let mut rendezvous = connect_tcp(&request.server.id_server, RENDEZVOUS_PORT)?;

    progress(45, "Connecting to Relay server".to_owned());
    let _relay = connect_tcp(&request.server.relay_server, RELAY_PORT)?;

    progress(60, "Sending RustDesk PunchHoleRequest protobuf".to_owned());
    let message = RendezvousMessage {
        union: Some(rendezvous_message::Union::PunchHoleRequest(
            PunchHoleRequest {
                id: request.remote_id.clone(),
                nat_type: NatType::UnknownNat as i32,
                licence_key: request.server.public_key.clone(),
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
    send_framed(&mut rendezvous, &encode_message(&message))?;

    progress(80, "Waiting for rendezvous response".to_owned());
    rendezvous
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|err| format!("Failed to set read timeout: {err}"))?;
    let response = read_framed(&mut rendezvous)?;
    let decoded = decode_message(&response).map_err(|err| format!("Decode failed: {err}"))?;
    let rendezvous = describe_rendezvous_response(&decoded)?;

    progress(85, "Rendezvous protobuf response decoded".to_owned());
    let relay_server = rendezvous
        .relay_server
        .unwrap_or_else(|| request.server.relay_server.clone());
    let secure_relay = rendezvous.has_signed_pk;

    progress(88, "Requesting relay reservation".to_owned());
    let relay_uuid = request_relay_reservation(
        &request.server.id_server,
        &request.remote_id,
        &relay_server,
        &request.server.public_key,
        secure_relay,
    )?;

    progress(92, "Opening relay stream".to_owned());
    let mut relay_stream = open_relay_stream(
        &relay_server,
        &request.remote_id,
        &relay_uuid,
        &request.server.public_key,
        secure_relay,
    )?;

    progress(96, "Waiting for peer secure/login response".to_owned());
    let (peer_stage, displays) =
        read_initial_peer_stage(&mut relay_stream, &request.password, &request.remote_id)?;

    Ok((relay_stream, peer_stage, displays))
}

fn validate_public_key(public_key: &str) -> Result<(), String> {
    let decoded = STANDARD
        .decode(public_key)
        .map_err(|err| format!("Invalid public key base64: {err}"))?;
    if decoded.len() != 32 {
        return Err(format!(
            "Invalid public key length: expected 32 bytes, got {}",
            decoded.len()
        ));
    }
    Ok(())
}

fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, String> {
    let mut last_error = None;
    let (host, port) = split_host_port(host, port);
    let addrs = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|err| format!("{host}:{port}: DNS error: {err}"))?;

    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_error = Some(err),
        }
    }

    Err(format!(
        "{host}:{port} unreachable: {}",
        last_error
            .map(|err| err.to_string())
            .unwrap_or_else(|| "no resolved addresses".to_owned())
    ))
}

fn split_host_port(host: &str, default_port: u16) -> (String, u16) {
    let trimmed = host.trim();
    if let Some((name, port)) = trimmed.rsplit_once(':') {
        if !name.is_empty() && !name.contains(']') {
            if let Ok(port) = port.parse::<u16>() {
                return (name.to_owned(), port);
            }
        }
    }
    (trimmed.to_owned(), default_port)
}

fn send_framed(stream: &mut TcpStream, payload: &[u8]) -> Result<(), String> {
    let mut out = encode_frame_len(payload.len())?;
    out.extend_from_slice(payload);
    stream
        .write_all(&out)
        .map_err(|err| format!("TCP write failed: {err}"))
}

fn read_framed(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut first = [0_u8; 1];
    stream
        .read_exact(&mut first)
        .map_err(|err| format!("TCP read header failed: {err}"))?;
    let head_len = ((first[0] & 0x3) + 1) as usize;
    let mut header = vec![0_u8; head_len];
    header[0] = first[0];
    if head_len > 1 {
        stream
            .read_exact(&mut header[1..])
            .map_err(|err| format!("TCP read header failed: {err}"))?;
    }

    let mut len = header[0] as usize;
    if head_len > 1 {
        len |= (header[1] as usize) << 8;
    }
    if head_len > 2 {
        len |= (header[2] as usize) << 16;
    }
    if head_len > 3 {
        len |= (header[3] as usize) << 24;
    }
    len >>= 2;

    let mut payload = vec![0_u8; len];
    stream
        .read_exact(&mut payload)
        .map_err(|err| format!("TCP read payload failed: {err}"))?;
    Ok(payload)
}

fn encode_frame_len(len: usize) -> Result<Vec<u8>, String> {
    if len <= 0x3f {
        Ok(vec![(len << 2) as u8])
    } else if len <= 0x3fff {
        Ok(((len << 2) as u16 | 0x1).to_le_bytes().to_vec())
    } else if len <= 0x3fffff {
        let header = (len << 2) as u32 | 0x2;
        Ok(vec![
            (header & 0xff) as u8,
            ((header >> 8) & 0xff) as u8,
            ((header >> 16) & 0xff) as u8,
        ])
    } else if len <= 0x3fffffff {
        Ok(((len << 2) as u32 | 0x3).to_le_bytes().to_vec())
    } else {
        Err("Frame too large".to_owned())
    }
}

fn request_relay_reservation(
    rendezvous_server: &str,
    remote_id: &str,
    relay_server: &str,
    public_key: &str,
    secure: bool,
) -> Result<String, String> {
    let mut socket = connect_tcp(rendezvous_server, RENDEZVOUS_PORT)?;
    let uuid = uuid::Uuid::new_v4().to_string();
    let request = RendezvousMessage {
        union: Some(rendezvous_message::Union::RequestRelay(RequestRelay {
            id: remote_id.to_owned(),
            uuid: uuid.clone(),
            socket_addr: Vec::new(),
            relay_server: relay_server.to_owned(),
            secure,
            licence_key: public_key.to_owned(),
            conn_type: ConnType::DefaultConn as i32,
            token: String::new(),
        })),
    };
    send_framed(&mut socket, &encode_message(&request))?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|err| format!("Failed to set read timeout: {err}"))?;
    let response = decode_message(&read_framed(&mut socket)?)
        .map_err(|err| format!("Relay reservation decode failed: {err}"))?;
    match response.union {
        Some(rendezvous_message::Union::RelayResponse(response)) => {
            if response.refuse_reason.is_empty() {
                Ok(uuid)
            } else {
                Err(response.refuse_reason)
            }
        }
        _ => Err("Unexpected relay reservation response".to_owned()),
    }
}

fn open_relay_stream(
    relay_server: &str,
    remote_id: &str,
    relay_uuid: &str,
    public_key: &str,
    secure: bool,
) -> Result<TcpStream, String> {
    let mut relay = connect_tcp(relay_server, RELAY_PORT)?;
    let request = RendezvousMessage {
        union: Some(rendezvous_message::Union::RequestRelay(RequestRelay {
            id: remote_id.to_owned(),
            uuid: relay_uuid.to_owned(),
            socket_addr: Vec::new(),
            relay_server: String::new(),
            secure,
            licence_key: public_key.to_owned(),
            conn_type: ConnType::DefaultConn as i32,
            token: String::new(),
        })),
    };
    send_framed(&mut relay, &encode_message(&request))?;
    Ok(relay)
}

fn read_initial_peer_stage(
    relay: &mut TcpStream,
    password: &str,
    remote_id: &str,
) -> Result<(String, Vec<RemoteDisplay>), String> {
    relay
        .set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|err| format!("Failed to set relay read timeout: {err}"))?;
    let mut sent_login = false;
    for _ in 0..12 {
        let payload = read_framed(relay).map_err(|err| {
            format!("Relay opened, but no peer secure/login message arrived: {err}")
        })?;
        let message = decode_peer_message(&payload)
            .map_err(|err| format!("Peer message decode failed: {err}"))?;

        match message.union {
            Some(peer_message::Union::SignedId(_)) => {
                let fallback = PeerMessage {
                    union: Some(peer_message::Union::PublicKey(PublicKey {
                        asymmetric_value: Vec::new(),
                        symmetric_value: Vec::new(),
                    })),
                };
                send_framed(relay, &encode_peer_message(&fallback))?;
            }
            Some(peer_message::Union::Hash(hash)) => {
                let login = build_login_request(password, &hash.salt, &hash.challenge, remote_id);
                send_framed(relay, &encode_peer_message(&login))?;
                sent_login = true;
            }
            Some(peer_message::Union::LoginResponse(response)) => {
                send_selected_windows_session(relay, &response)?;
                let displays = displays_from_login_response(&response);
                let login = describe_login_response(response, sent_login)?;
                send_video_start_messages(relay)?;
                return Ok((
                    format!("{login}; screenshot/control channel ready"),
                    displays,
                ));
            }
            Some(peer_message::Union::PeerInfo(info)) => {
                send_selected_windows_session_from_peer_info(relay, &info)?;
                let login = format!(
                    "authorized; peer info received: {} {} {}",
                    info.hostname, info.platform, info.version
                );
                let displays = displays_from_peer_info(&info);
                send_video_start_messages(relay)?;
                return Ok((
                    format!("{login}; screenshot/control channel ready"),
                    displays,
                ));
            }
            Some(peer_message::Union::PublicKey(_)) => {}
            Some(peer_message::Union::TestDelay(delay)) => {
                echo_test_delay(relay, delay)?;
            }
            Some(peer_message::Union::Misc(_)) => {}
            Some(peer_message::Union::MouseEvent(_))
            | Some(peer_message::Union::KeyEvent(_))
            | Some(peer_message::Union::ScreenshotRequest(_))
            | Some(peer_message::Union::ScreenshotResponse(_)) => {}
            Some(peer_message::Union::VideoFrame(frame)) => {
                return Ok((
                    format!(
                        "video before login response: {}",
                        describe_video_frame(&frame)
                    ),
                    Vec::new(),
                ));
            }
            Some(peer_message::Union::LoginRequest(_)) => {
                return Err("Unexpected login-request received from peer".to_owned());
            }
            None => {
                // RustDesk can send an empty message while falling back from secure
                // negotiation. Answer with an empty public-key fallback and keep
                // reading until the login/hash/video stage appears.
                let fallback = PeerMessage {
                    union: Some(peer_message::Union::PublicKey(PublicKey {
                        asymmetric_value: Vec::new(),
                        symmetric_value: Vec::new(),
                    })),
                };
                send_framed(relay, &encode_peer_message(&fallback))?;
            }
        }
    }
    Err("Login response timeout after peer handshake".to_owned())
}

fn send_video_start_messages(relay: &mut TcpStream) -> Result<(), String> {
    let refresh_all = PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::RefreshVideo(true)),
        })),
    };
    send_framed(relay, &encode_peer_message(&refresh_all))?;

    let refresh_display = PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::RefreshVideoDisplay(0)),
        })),
    };
    send_framed(relay, &encode_peer_message(&refresh_display))
}

fn send_selected_windows_session(
    relay: &mut TcpStream,
    response: &crate::rustdesk_proto::LoginResponse,
) -> Result<(), String> {
    if let Some(crate::rustdesk_proto::login_response::Union::PeerInfo(info)) = &response.union {
        send_selected_windows_session_from_peer_info(relay, info)?;
    }
    Ok(())
}

fn send_selected_windows_session_from_peer_info(
    relay: &mut TcpStream,
    info: &crate::rustdesk_proto::PeerInfo,
) -> Result<(), String> {
    let Some(windows_sessions) = &info.windows_sessions else {
        return Ok(());
    };
    if windows_sessions.sessions.is_empty() || windows_sessions.current_sid == 0 {
        return Ok(());
    }
    let message = PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::SelectedSid(windows_sessions.current_sid)),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

#[allow(dead_code)]
fn wait_for_video_probe(relay: &mut TcpStream) -> Result<String, String> {
    relay
        .set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|err| format!("Failed to set relay read timeout: {err}"))?;

    for attempt in 0..24 {
        let payload = match read_framed(relay) {
            Ok(payload) => payload,
            Err(_) if attempt < 23 => {
                send_video_start_messages(relay)?;
                continue;
            }
            Err(err) => {
                return Err(format!(
                    "Authorized, but no video/control message arrived: {err}"
                ));
            }
        };
        let message = decode_peer_message(&payload)
            .map_err(|err| format!("Post-login message decode failed: {err}"))?;
        match message.union {
            Some(peer_message::Union::VideoFrame(frame)) => {
                send_video_received(relay)?;
                return Ok(format!(
                    "first video frame: {}",
                    describe_video_frame(&frame)
                ));
            }
            Some(peer_message::Union::PeerInfo(info)) => {
                let displays = info
                    .displays
                    .iter()
                    .map(|d| format!("{}x{}", d.width, d.height))
                    .collect::<Vec<_>>()
                    .join(", ");
                if !displays.is_empty() {
                    return Ok(format!("peer displays: {displays}; waiting for video next"));
                }
            }
            Some(peer_message::Union::LoginResponse(response)) => {
                send_selected_windows_session(relay, &response)?;
                let _ = describe_login_response(response, true)?;
            }
            Some(peer_message::Union::TestDelay(delay)) => {
                echo_test_delay(relay, delay)?;
            }
            Some(peer_message::Union::Misc(_)) => {}
            Some(peer_message::Union::Hash(_))
            | Some(peer_message::Union::SignedId(_))
            | Some(peer_message::Union::PublicKey(_))
            | Some(peer_message::Union::LoginRequest(_))
            | Some(peer_message::Union::MouseEvent(_))
            | Some(peer_message::Union::KeyEvent(_))
            | Some(peer_message::Union::ScreenshotRequest(_))
            | Some(peer_message::Union::ScreenshotResponse(_))
            | None => {}
        }
    }

    Ok("authorized; no video frame received during probe window".to_owned())
}

fn echo_test_delay(
    relay: &mut TcpStream,
    mut delay: crate::rustdesk_proto::TestDelay,
) -> Result<(), String> {
    if delay.from_client {
        return Ok(());
    }
    delay.from_client = true;
    let message = PeerMessage {
        union: Some(peer_message::Union::TestDelay(delay)),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn send_video_received(relay: &mut TcpStream) -> Result<(), String> {
    let message = PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::VideoReceived(true)),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn handle_session_message(
    message: PeerMessage,
    relay: &mut TcpStream,
    events: &Sender<SessionEvent>,
    frame_tx: &SyncSender<DecoderInput>,
) -> bool {
    match message.union {
        Some(peer_message::Union::ScreenshotResponse(response)) => {
            if response.msg.is_empty() && !response.data.is_empty() {
                match frame_tx.try_send(DecoderInput::Png {
                    sid: response.sid,
                    png: response.data,
                }) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => {
                        let _ = events.send(SessionEvent::Info(
                            "Frame decoder stopped unexpectedly".to_owned(),
                        ));
                    }
                };
                true
            } else if !response.msg.is_empty() {
                let _ = events.send(SessionEvent::Info(format!(
                    "Screenshot failed: {}",
                    response.msg
                )));
                true
            } else {
                true
            }
        }
        Some(peer_message::Union::VideoFrame(frame)) => {
            let _ = send_video_received(relay);
            if let Some(video_frame::Union::H264s(frames)) = frame.union {
                let sid = frames
                    .frames
                    .last()
                    .map(|frame| format!("h264-{}", frame.pts))
                    .unwrap_or_else(|| "h264".to_owned());
                match frame_tx.try_send(DecoderInput::H264 { sid, frames }) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => {
                        let _ = events.send(SessionEvent::Info(
                            "Frame decoder stopped unexpectedly".to_owned(),
                        ));
                    }
                }
            }
            false
        }
        Some(peer_message::Union::TestDelay(delay)) => {
            let _ = echo_test_delay(relay, delay);
            false
        }
        Some(peer_message::Union::LoginResponse(response)) => {
            emit_displays_from_login_response(&response, events);
            let _ = send_selected_windows_session(relay, &response);
            false
        }
        Some(peer_message::Union::PeerInfo(info)) => {
            emit_displays_from_peer_info(&info, events);
            let _ = send_selected_windows_session_from_peer_info(relay, &info);
            false
        }
        Some(peer_message::Union::Hash(_))
        | Some(peer_message::Union::SignedId(_))
        | Some(peer_message::Union::PublicKey(_))
        | Some(peer_message::Union::LoginRequest(_))
        | Some(peer_message::Union::Misc(_))
        | Some(peer_message::Union::MouseEvent(_))
        | Some(peer_message::Union::KeyEvent(_))
        | Some(peer_message::Union::ScreenshotRequest(_))
        | None => false,
    }
}

fn request_screenshot_if_idle(
    relay: &mut TcpStream,
    counter: &mut u64,
    display: i32,
    pending: &mut bool,
    events: &Sender<SessionEvent>,
    received: u64,
) {
    if *pending {
        return;
    }
    match request_screenshot(relay, counter, display) {
        Ok(()) => {
            *pending = true;
            let _ = events.send(SessionEvent::ScreenshotStats {
                received,
                pending: true,
            });
        }
        Err(err) => {
            let _ = events.send(SessionEvent::Info(format!(
                "Screenshot request failed: {err}"
            )));
        }
    }
}

fn emit_displays_from_login_response(
    response: &crate::rustdesk_proto::LoginResponse,
    events: &Sender<SessionEvent>,
) {
    if let Some(crate::rustdesk_proto::login_response::Union::PeerInfo(info)) = &response.union {
        emit_displays_from_peer_info(info, events);
    }
}

fn emit_displays_from_peer_info(
    info: &crate::rustdesk_proto::PeerInfo,
    events: &Sender<SessionEvent>,
) {
    let displays = displays_from_peer_info(info);
    if !displays.is_empty() {
        let _ = events.send(SessionEvent::Displays(displays));
    }
}

fn displays_from_login_response(
    response: &crate::rustdesk_proto::LoginResponse,
) -> Vec<RemoteDisplay> {
    if let Some(crate::rustdesk_proto::login_response::Union::PeerInfo(info)) = &response.union {
        return displays_from_peer_info(info);
    }
    Vec::new()
}

fn displays_from_peer_info(info: &crate::rustdesk_proto::PeerInfo) -> Vec<RemoteDisplay> {
    let displays = info
        .displays
        .iter()
        .enumerate()
        .map(|(index, display)| RemoteDisplay {
            index: index as i32,
            name: if display.name.is_empty() {
                format!("Display {}", index + 1)
            } else {
                display.name.clone()
            },
            width: display.width,
            height: display.height,
            x: display.x,
            y: display.y,
            cursor_embedded: display.cursor_embedded,
        })
        .collect::<Vec<_>>();
    displays
}

fn request_screenshot(
    relay: &mut TcpStream,
    counter: &mut u64,
    display: i32,
) -> Result<(), String> {
    *counter += 1;
    let message = PeerMessage {
        union: Some(peer_message::Union::ScreenshotRequest(ScreenshotRequest {
            display,
            sid: format!("evertydesk-lite-{counter}"),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn decode_frame_loop(frame_rx: Receiver<DecoderInput>, events: Sender<SessionEvent>) {
    #[cfg(feature = "live-h264")]
    let mut h264 = openh264::decoder::Decoder::new().ok();

    while let Ok(frame) = frame_rx.recv() {
        let result = match frame {
            DecoderInput::Png { sid, png } => {
                decode_png_rgba(&png).map(|(width, height, rgba)| SessionEvent::Frame {
                    sid,
                    width,
                    height,
                    rgba,
                })
            }
            DecoderInput::H264 { sid, frames } => {
                #[cfg(feature = "live-h264")]
                {
                    decode_h264_rgba(h264.as_mut(), frames).map(|(width, height, rgba)| {
                        SessionEvent::Frame {
                            sid,
                            width,
                            height,
                            rgba,
                        }
                    })
                }
                #[cfg(not(feature = "live-h264"))]
                {
                    let _ = sid;
                    let _ = frames;
                    Err(
                        "H264 frame received, but this build was compiled without live-h264"
                            .to_owned(),
                    )
                }
            }
        };

        match result {
            Ok(event) => {
                let _ = events.send(event);
            }
            Err(err) => {
                let _ = events.send(SessionEvent::Info(format!("Frame decode failed: {err}")));
            }
        }
    }
}

#[cfg(feature = "live-h264")]
fn decode_h264_rgba(
    decoder: Option<&mut openh264::decoder::Decoder>,
    frames: EncodedVideoFrames,
) -> Result<(usize, usize, Vec<u8>), String> {
    let decoder = decoder.ok_or_else(|| "OpenH264 decoder init failed".to_owned())?;
    let mut decoded = None;
    for frame in frames.frames {
        if frame.data.is_empty() {
            continue;
        }
        decoded = decoder
            .decode(&frame.data)
            .map_err(|err| err.to_string())?
            .map(|yuv| {
                let (width, height) = yuv.dimensions();
                let mut rgba = vec![0; width * height * 4];
                yuv.write_rgba8(&mut rgba);
                (width, height, rgba)
            })
            .or(decoded);
    }
    decoded.ok_or_else(|| "H264 decoder needs more packets".to_owned())
}

fn decode_png_rgba(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    let image = image::load_from_memory(bytes)
        .map_err(|err| err.to_string())?
        .to_rgba8();
    Ok((
        image.width() as usize,
        image.height() as usize,
        image.into_raw(),
    ))
}

fn send_switch_display(
    relay: &mut TcpStream,
    display: i32,
    info: Option<&RemoteDisplay>,
) -> Result<(), String> {
    let switch_display = SwitchDisplay {
        display,
        x: info.map(|d| d.x).unwrap_or_default(),
        y: info.map(|d| d.y).unwrap_or_default(),
        width: info.map(|d| d.width).unwrap_or_default(),
        height: info.map(|d| d.height).unwrap_or_default(),
        cursor_embedded: info.map(|d| d.cursor_embedded).unwrap_or_default(),
    };
    let message = PeerMessage {
        union: Some(peer_message::Union::Misc(Misc {
            union: Some(misc::Union::SwitchDisplay(switch_display)),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn send_mouse(relay: &mut TcpStream, mask: i32, x: i32, y: i32) -> Result<(), String> {
    let message = PeerMessage {
        union: Some(peer_message::Union::MouseEvent(MouseEvent {
            mask,
            x,
            y,
            modifiers: Vec::new(),
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn send_text(relay: &mut TcpStream, text: &str) -> Result<(), String> {
    send_text_with_modifiers(relay, text, &[])
}

fn send_text_with_modifiers(
    relay: &mut TcpStream,
    text: &str,
    modifiers: &[ControlKey],
) -> Result<(), String> {
    for ch in text.chars() {
        send_key(
            relay,
            crate::rustdesk_proto::key_event::Union::Unicode(ch as u32),
            modifiers,
        )?;
    }
    Ok(())
}

fn send_control_key(relay: &mut TcpStream, key: ControlKey) -> Result<(), String> {
    send_control_key_with_modifiers(relay, key, &[])
}

fn send_control_key_with_modifiers(
    relay: &mut TcpStream,
    key: ControlKey,
    modifiers: &[ControlKey],
) -> Result<(), String> {
    send_key(
        relay,
        crate::rustdesk_proto::key_event::Union::ControlKey(key as i32),
        modifiers,
    )
}

fn send_key(
    relay: &mut TcpStream,
    union: crate::rustdesk_proto::key_event::Union,
    modifiers: &[ControlKey],
) -> Result<(), String> {
    let message = PeerMessage {
        union: Some(peer_message::Union::KeyEvent(KeyEvent {
            down: false,
            press: true,
            union: Some(union),
            modifiers: modifiers.iter().map(|key| *key as i32).collect(),
            mode: KeyboardMode::Legacy as i32,
        })),
    };
    send_framed(relay, &encode_peer_message(&message))
}

fn is_timeout_error(err: &str) -> bool {
    err.contains("timed out")
        || err.contains("would block")
        || err.contains("10060")
        || err.contains("Попытка установить соединение")
}

fn describe_video_frame(frame: &crate::rustdesk_proto::VideoFrame) -> String {
    match &frame.union {
        Some(video_frame::Union::Rgb(rgb)) => {
            format!("display {} RGB compress={}", frame.display, rgb.compress)
        }
        Some(video_frame::Union::Yuv(yuv)) => format!(
            "display {} YUV compress={} stride={}",
            frame.display, yuv.compress, yuv.stride
        ),
        Some(video_frame::Union::Vp8s(frames)) => {
            format!(
                "display {} VP8 frames={}",
                frame.display,
                frames.frames.len()
            )
        }
        Some(video_frame::Union::Vp9s(frames)) => {
            format!(
                "display {} VP9 frames={}",
                frame.display,
                frames.frames.len()
            )
        }
        Some(video_frame::Union::H264s(frames)) => {
            format!(
                "display {} H264 frames={}",
                frame.display,
                frames.frames.len()
            )
        }
        Some(video_frame::Union::H265s(frames)) => {
            format!(
                "display {} H265 frames={}",
                frame.display,
                frames.frames.len()
            )
        }
        Some(video_frame::Union::Av1s(frames)) => {
            format!(
                "display {} AV1 frames={}",
                frame.display,
                frames.frames.len()
            )
        }
        None => format!("display {} empty video frame", frame.display),
    }
}

fn describe_login_response(
    response: crate::rustdesk_proto::LoginResponse,
    sent_login: bool,
) -> Result<String, String> {
    match response.union {
        Some(crate::rustdesk_proto::login_response::Union::Error(err)) => {
            Err(format!("Login refused: {}", describe_login_error(&err)))
        }
        Some(crate::rustdesk_proto::login_response::Union::PeerInfo(info)) => {
            let prefix = if sent_login {
                "authorized"
            } else {
                "peer accepted without password hash"
            };
            Ok(format!(
                "{prefix}; peer info: hostname={}, platform={}, version={}",
                info.hostname, info.platform, info.version
            ))
        }
        None => Ok("empty login response".to_owned()),
    }
}

fn describe_login_error(error: &str) -> String {
    if error.to_ascii_lowercase().contains("wrong password") {
        "Wrong Password".to_owned()
    } else {
        error.to_owned()
    }
}

fn build_login_request(
    password: &str,
    salt: &str,
    challenge: &str,
    remote_id: &str,
) -> PeerMessage {
    let password_hash = if password.is_empty() {
        Vec::new()
    } else {
        let mut h1 = Sha256::new();
        h1.update(password.as_bytes());
        h1.update(salt.as_bytes());
        let h1 = h1.finalize();

        let mut h2 = Sha256::new();
        h2.update(h1);
        h2.update(challenge.as_bytes());
        h2.finalize().to_vec()
    };

    PeerMessage {
        union: Some(peer_message::Union::LoginRequest(LoginRequest {
            username: remote_id.to_owned(),
            password: password_hash,
            my_id: "evertydesk-lite".to_owned(),
            my_name: "EvertyDesk Lite".to_owned(),
            option: Some(OptionMessage {
                supported_decoding: Some(SupportedDecoding {
                    ability_vp9: 0,
                    ability_h264: i32::from(cfg!(feature = "live-h264")),
                    ability_h265: 0,
                    prefer: if cfg!(feature = "live-h264") {
                        PreferCodec::H264 as i32
                    } else {
                        PreferCodec::Auto as i32
                    },
                    ability_vp8: 0,
                    ability_av1: 0,
                    i444: Some(CodecAbility {
                        vp8: false,
                        vp9: false,
                        av1: false,
                        h264: cfg!(feature = "live-h264"),
                        h265: false,
                    }),
                    prefer_chroma: Chroma::I420 as i32,
                }),
            }),
            video_ack_required: false,
            version: "1.4.6".to_owned(),
            my_platform: std::env::consts::OS.to_owned(),
        })),
    }
}

struct RendezvousInfo {
    relay_server: Option<String>,
    has_signed_pk: bool,
}

fn describe_rendezvous_response(message: &RendezvousMessage) -> Result<RendezvousInfo, String> {
    match &message.union {
        Some(rendezvous_message::Union::PunchHoleResponse(response)) => {
            if !response.other_failure.is_empty() {
                return Err(response.other_failure.clone());
            }
            if response.socket_addr.is_empty() && response.relay_server.is_empty() {
                let failure = PunchHoleFailure::try_from(response.failure)
                    .map(describe_punch_hole_failure)
                    .unwrap_or_else(|_| format!("unknown failure {}", response.failure));
                return Err(format!("Rendezvous refused: {failure}"));
            }
            Ok(RendezvousInfo {
                relay_server: (!response.relay_server.is_empty())
                    .then(|| response.relay_server.clone()),
                has_signed_pk: !response.pk.is_empty(),
            })
        }
        Some(rendezvous_message::Union::RelayResponse(response)) => {
            if !response.refuse_reason.is_empty() {
                Err(response.refuse_reason.clone())
            } else {
                Ok(RendezvousInfo {
                    relay_server: (!response.relay_server.is_empty())
                        .then(|| response.relay_server.clone()),
                    has_signed_pk: false,
                })
            }
        }
        Some(rendezvous_message::Union::PunchHoleRequest(_)) => {
            Err("Unexpected PunchHoleRequest response".to_owned())
        }
        Some(rendezvous_message::Union::RequestRelay(_)) => {
            Err("Unexpected RequestRelay response".to_owned())
        }
        Some(rendezvous_message::Union::OnlineRequest(_)) => {
            Err("Unexpected OnlineRequest response".to_owned())
        }
        Some(rendezvous_message::Union::OnlineResponse(_)) => {
            Err("Unexpected OnlineResponse response".to_owned())
        }
        None => Err("Empty rendezvous response".to_owned()),
    }
}

fn describe_punch_hole_failure(failure: PunchHoleFailure) -> String {
    match failure {
        PunchHoleFailure::IdNotExist => "ID does not exist on ID server".to_owned(),
        PunchHoleFailure::Offline => {
            "Offline: remote ID is not connected to this ID server now".to_owned()
        }
        PunchHoleFailure::LicenseMismatch => "License/public key mismatch".to_owned(),
        PunchHoleFailure::LicenseOveruse => "License/session limit exceeded".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_len_matches_rustdesk_codec_short_packet() {
        assert_eq!(encode_frame_len(0).unwrap(), vec![0]);
        assert_eq!(encode_frame_len(1).unwrap(), vec![4]);
        assert_eq!(encode_frame_len(0x3f).unwrap(), vec![0xfc]);
    }

    #[test]
    fn frame_len_matches_rustdesk_codec_medium_packet() {
        assert_eq!(encode_frame_len(0x40).unwrap(), vec![0x01, 0x01]);
        assert_eq!(encode_frame_len(0x3fff).unwrap(), vec![0xfd, 0xff]);
    }

    #[test]
    fn everty_public_key_is_valid_ed25519_size() {
        validate_public_key("MrGdbay3g8Qr84YYnxr4qLjw5zLWM1oAOdfehbBnlRs=").unwrap();
    }

    #[test]
    fn split_host_port_accepts_explicit_port() {
        assert_eq!(
            split_host_port("edesk.server1.everty.ru:21117", 21117),
            ("edesk.server1.everty.ru".to_owned(), 21117)
        );
    }

    #[test]
    fn split_host_port_uses_default_when_missing() {
        assert_eq!(
            split_host_port("edesk.server1.everty.ru", 21117),
            ("edesk.server1.everty.ru".to_owned(), 21117)
        );
    }

    #[test]
    fn login_request_uses_32_byte_password_hash() {
        let message = build_login_request("secret", "salt", "challenge", "123");
        let Some(peer_message::Union::LoginRequest(login)) = message.union else {
            panic!("expected login request");
        };
        assert_eq!(login.password.len(), 32);
        assert_eq!(login.username, "123");
    }

    #[test]
    fn login_request_uses_empty_password_for_remote_approval() {
        let message = build_login_request("", "salt", "challenge", "123");
        let Some(peer_message::Union::LoginRequest(login)) = message.union else {
            panic!("expected login request");
        };
        assert!(login.password.is_empty());
        assert_eq!(login.username, "123");
    }

    #[test]
    fn login_response_peer_info_is_success() {
        let response = crate::rustdesk_proto::LoginResponse {
            union: Some(crate::rustdesk_proto::login_response::Union::PeerInfo(
                crate::rustdesk_proto::PeerInfo {
                    username: "user".to_owned(),
                    hostname: "host".to_owned(),
                    platform: "windows".to_owned(),
                    displays: Vec::new(),
                    current_display: 0,
                    version: "1.4.6".to_owned(),
                    windows_sessions: None,
                },
            )),
        };
        let text = describe_login_response(response, true).unwrap();
        assert!(text.contains("authorized"));
        assert!(text.contains("host"));
    }

    #[test]
    fn login_response_error_is_failure() {
        let response = crate::rustdesk_proto::LoginResponse {
            union: Some(crate::rustdesk_proto::login_response::Union::Error(
                "Wrong Password".to_owned(),
            )),
        };
        assert!(describe_login_response(response, true)
            .unwrap_err()
            .contains("Wrong Password"));
    }

    #[test]
    fn video_frame_description_reports_codec() {
        let frame = crate::rustdesk_proto::VideoFrame {
            display: 0,
            union: Some(crate::rustdesk_proto::video_frame::Union::H264s(
                crate::rustdesk_proto::EncodedVideoFrames {
                    frames: vec![crate::rustdesk_proto::EncodedVideoFrame {
                        data: vec![1, 2, 3],
                        key: true,
                        pts: 42,
                    }],
                },
            )),
        };
        assert!(describe_video_frame(&frame).contains("H264"));
    }
}
