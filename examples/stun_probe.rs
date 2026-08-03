//! Live sanity check for STUN public-endpoint discovery — EVRT2 WAN fix.
//!
//! Run with: cargo run --example stun_probe
//!
//! Binds a real UDP socket (like try_open_evrt_socket does), queries the
//! public STUN servers, and prints whatever public ip:port comes back.
//! Requires actual internet access — this is a live network test, not a unit test.

use evertydesk_core::netif;
use std::net::UdpSocket;

fn main() {
    let sock = UdpSocket::bind("0.0.0.0:0").expect("bind local UDP socket");
    let local_addr = sock.local_addr().unwrap();
    println!("Local socket bound: {local_addr}");
    println!("Querying STUN servers for public-facing address...\n");

    match netif::discover_public_endpoint(&sock) {
        Some(public_addr) => {
            println!("SUCCESS — public address as seen from outside NAT: {public_addr}");
            println!(
                "\nLocal port was {}, public port is {} — {}",
                local_addr.port(),
                public_addr.port(),
                if local_addr.port() == public_addr.port() {
                    "SAME (full-cone/no NAT, or 1:1 port mapping — best case for direct UDP)"
                } else {
                    "DIFFERENT (NAT is remapping the port — still fine unless it's symmetric NAT, in which case this exact port won't match what a peer sees)"
                }
            );
        }
        None => {
            println!("FAILED — no STUN server responded (no internet, firewall blocking outbound UDP/3478/19302, or all three servers unreachable).");
        }
    }
}
