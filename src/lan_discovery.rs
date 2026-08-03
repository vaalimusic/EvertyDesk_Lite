//! LAN peer discovery — compatible with RustDesk's protocol.
//!
//! The viewer (Android/Windows RustDesk client) broadcasts a UDP
//! `PeerDiscovery { cmd="ping" }` on port 21119 (RENDEZVOUS_PORT + 3).
//! This module listens on that port and replies with
//! `PeerDiscovery { cmd="pong", id, hostname, platform, ... }`
//! so the viewer can find the host without going through the relay server.

use std::{
    net::{SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use prost::Message as _;

use crate::{
    rustdesk_proto::{rendezvous_message, PeerDiscovery, RendezvousMessage},
    settings::AppConfig,
};

const LAN_DISCOVERY_PORT: u16 = 21119; // RENDEZVOUS_PORT (21116) + 3

pub fn start(config: Arc<std::sync::Mutex<AppConfig>>, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("lan-discovery".into())
        .spawn(move || run(config, stop))
        .ok();
}

fn run(config: Arc<std::sync::Mutex<AppConfig>>, stop: Arc<AtomicBool>) {
    let socket = match UdpSocket::bind(format!("0.0.0.0:{LAN_DISCOVERY_PORT}")) {
        Ok(s) => s,
        Err(e) => {
            log(format!(
                "LAN discovery: bind port {LAN_DISCOVERY_PORT} failed: {e}"
            ));
            return;
        }
    };
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok();
    log(format!(
        "LAN discovery: listening on UDP {LAN_DISCOVERY_PORT}"
    ));

    let mut buf = vec![0u8; 2048];
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match socket.recv_from(&mut buf) {
            Ok((len, src)) => {
                if let Ok(msg) = RendezvousMessage::decode(&buf[..len]) {
                    if let Some(rendezvous_message::Union::PeerDiscovery(p)) = msg.union {
                        handle_ping(p, src, &socket, &config);
                    }
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => {
                log(format!("LAN discovery: recv error: {e}"));
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
    log("LAN discovery: stopped".to_owned());
}

fn handle_ping(
    ping: PeerDiscovery,
    src: SocketAddr,
    socket: &UdpSocket,
    config: &Arc<std::sync::Mutex<AppConfig>>,
) {
    if ping.cmd != "ping" {
        return;
    }
    let cfg = config.lock().unwrap();
    let local_id = cfg.local_id.clone();
    drop(cfg);

    // Don't respond to our own ping (in case viewer runs on the same machine)
    if ping.id == local_id {
        return;
    }

    let hostname = hostname();
    let pong = RendezvousMessage {
        union: Some(rendezvous_message::Union::PeerDiscovery(PeerDiscovery {
            cmd: "pong".to_owned(),
            id: local_id,
            hostname,
            username: current_username(),
            platform: "Windows".to_owned(),
            mac: String::new(),
            misc: String::new(),
        })),
    };

    let bytes = pong.encode_to_vec();
    if let Err(e) = socket.send_to(&bytes, src) {
        log(format!("LAN discovery: pong send failed: {e}"));
    } else {
        log(format!(
            "LAN discovery: pong → {src} (ping from {})",
            ping.id
        ));
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "EvertyDesk".to_owned())
}

fn current_username() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default()
}

fn log(msg: String) {
    // Lightweight — no channel needed, just stderr for debugging.
    // In production the host UI doesn't show LAN discovery logs.
    eprintln!("[lan] {msg}");
}
