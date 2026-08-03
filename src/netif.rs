// =============================================================================
// EVRT — перечисление локальных сетевых интерфейсов.
// Нужно для mini-ICE: хост отдаёт все свои IPv4 (LAN + VPN + ...) как
// кандидаты, клиент пробует каждый. Решает мультихоминг (VPN).
// =============================================================================

//! Перечисление локальных IPv4-адресов.
//!
//! Windows: `GetAdaptersAddresses`.
//! Unix (macOS/Linux): `getifaddrs`.
//! Возвращает только не-loopback IPv4, отсортированные так, чтобы
//! приватные LAN/VPN адреса шли первыми (они наиболее вероятно достижимы пиру).

use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Duration;

/// Вернуть все локальные IPv4-адреса (без loopback), отсортированные по
/// приоритету достижимости: приватные сети (LAN/VPN) → прочие.
pub fn local_ipv4_addresses() -> Vec<Ipv4Addr> {
    let mut addrs = platform_ipv4();
    // Убрать дубликаты
    addrs.sort();
    addrs.dedup();
    // Приоритет: приватные (LAN/VPN) первыми, link-local и прочее — в конце
    addrs.sort_by_key(|ip| priority(ip));
    addrs
}

/// Сформировать строку кандидатов "ip1:port,ip2:port,..." для Misc.
/// Исключаем link-local (169.254.x.x) — они почти всегда бесполезны и только
/// заставляют клиента тратить время на заведомо мёртвые попытки.
pub fn candidate_endpoints(port: u16) -> String {
    local_ipv4_addresses()
        .iter()
        .filter(|ip| !ip.is_link_local())
        .map(|ip| format!("{ip}:{port}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Меньше = выше приоритет.
fn priority(ip: &Ipv4Addr) -> u8 {
    let o = ip.octets();
    if ip.is_link_local() {
        3 // 169.254.x.x — почти всегда бесполезно
    } else if o[0] == 10
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
        || o[0] == 100
    // CGNAT / Tailscale 100.64.0.0/10
    {
        0 // приватные LAN/VPN — наивысший приоритет
    } else {
        1 // публичные/прочие
    }
}

// ─── Windows ──────────────────────────────────────────────────────────────────

#[cfg(all(windows, feature = "live-vp9-mf"))]
fn platform_ipv4() -> Vec<Ipv4Addr> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, AF_UNSPEC, SOCKADDR_IN};

    let mut out = Vec::new();
    unsafe {
        // Запрашиваем размер буфера
        let mut size: u32 = 0;
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let _ = GetAdaptersAddresses(AF_INET.0 as u32, flags, None, None, &mut size);
        if size == 0 {
            return out;
        }
        let mut buf = vec![0u8; size as usize];
        let head = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
        let ret = GetAdaptersAddresses(AF_INET.0 as u32, flags, None, Some(head), &mut size);
        if ret != 0 {
            return out;
        }

        let mut adapter = head;
        while !adapter.is_null() {
            let a = &*adapter;
            // Пропускаем выключенные интерфейсы
            let mut uni = a.FirstUnicastAddress;
            while !uni.is_null() {
                let u = &*uni;
                let sa = u.Address.lpSockaddr;
                if !sa.is_null() && (*sa).sa_family == AF_INET {
                    let sin = &*(sa as *const SOCKADDR_IN);
                    let b = sin.sin_addr.S_un.S_addr.to_ne_bytes();
                    let ip = Ipv4Addr::new(b[0], b[1], b[2], b[3]);
                    if !ip.is_loopback() && !ip.is_unspecified() {
                        out.push(ip);
                    }
                }
                uni = u.Next;
            }
            adapter = a.Next;
        }
        let _ = AF_UNSPEC;
    }
    out
}

#[cfg(all(windows, not(feature = "live-vp9-mf")))]
fn platform_ipv4() -> Vec<Ipv4Addr> {
    // Без windows-crate фичи — fallback на route-trick (один IP)
    route_trick()
}

// ─── Unix (macOS / Linux) ─────────────────────────────────────────────────────

#[cfg(unix)]
fn platform_ipv4() -> Vec<Ipv4Addr> {
    use std::ffi::CStr;
    use std::mem;

    let mut out = Vec::new();
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return route_trick();
        }
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null() {
                let sa = &*ifa.ifa_addr;
                if sa.sa_family as i32 == libc::AF_INET {
                    let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    let b = sin.sin_addr.s_addr.to_ne_bytes();
                    let ip = Ipv4Addr::new(b[0], b[1], b[2], b[3]);
                    // Пропускаем down-интерфейсы
                    let up = (ifa.ifa_flags as i32 & libc::IFF_UP) != 0;
                    if up && !ip.is_loopback() && !ip.is_unspecified() {
                        out.push(ip);
                    }
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
        let _ = (CStr::from_ptr, mem::size_of::<libc::ifaddrs>());
    }
    if out.is_empty() {
        route_trick()
    } else {
        out
    }
}

// ─── Fallback: route-trick (один primary IP) ─────────────────────────────────

#[cfg(not(any(unix, windows)))]
fn platform_ipv4() -> Vec<Ipv4Addr> {
    route_trick()
}

/// Узнать primary локальный IP через UDP connect к публичному адресу.
/// Не шлёт пакетов — только выбирает маршрут и читает local_addr.
#[cfg_attr(all(windows, feature = "live-vp9-mf"), allow(dead_code))]
fn route_trick() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("8.8.8.8:80").is_ok() {
            if let Ok(std::net::SocketAddr::V4(local)) = sock.local_addr() {
                let ip = *local.ip();
                if !ip.is_loopback() && !ip.is_unspecified() {
                    out.push(ip);
                }
            }
        }
    }
    out
}

// ─── STUN: публичный адрес через NAT (RFC 5389, минимальная реализация) ───────
//
// Зачем: candidate_endpoints() выше видит только ЛОКАЛЬНЫЕ интерфейсы (LAN/VPN).
// Если клиент не в той же LAN — punch не может пройти, EVRT-сессия падает на
// TCP relay (см. is_lan_ip / "EVRT: punch timeout" в evrt_session.rs/host.rs).
//
// STUN-сервер сообщает, каким наш адрес видно СНАРУЖИ NAT — этот адрес и есть
// недостающий publicly-reachable кандидат.
//
// КРИТИЧНО: проба должна идти через ТОТ ЖЕ сокет, на котором будет жить EVRT-
// сессия. NAT-маппинг (внешний ip:port) привязан к конкретной 5-tuple
// (протокол+локальный ip:port+удалённый ip:port); через отдельный сокет узнать
// бесполезно — порт не совпадёт с тем, что реально откроет NAT для EVRT-трафика.
//
// Ограничение: на symmetric NAT (внешний порт разный для разных удалённых
// адресов) обнаруженный STUN-адрес всё равно не будет совпадать с портом,
// который увидит клиент — punch не пройдёт. Это фундаментальное ограничение
// STUN, не баг: полное решение требует TURN/relay, который у нас уже есть как
// TCP relay fallback. STUN просто расширяет число случаев, где прямой UDP
// работает (full-cone / restricted-cone NAT — подавляющее большинство домашних
// роутеров), не пытается решить 100% случаев.

const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
const STUN_BINDING_REQUEST: u16 = 0x0001;
const STUN_BINDING_SUCCESS: u16 = 0x0101;
const STUN_ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const STUN_ATTR_MAPPED_ADDRESS: u16 = 0x0001;

/// Публичные STUN-серверы — пробуем по очереди, первый успешный ответ побеждает.
/// Все три реализуют RFC 5389/8489 Binding Request/Response, общедоступны без
/// аутентификации. Короткий таймаут на каждый — не задерживаем старт сессии
/// больше, чем на ~1.5с суммарно в худшем случае (все три недоступны).
const STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun.cloudflare.com:3478",
    "stun1.l.google.com:19302",
];

/// Обнаружить публичный (view-from-outside-NAT) адрес используя уже открытый
/// EVRT UDP-сокет. Best-effort: `None` при недоступности сети/всех STUN-
/// серверов — вызывающий код просто не добавляет публичный кандидат, всё
/// остальное (LAN-кандидаты, TCP relay fallback) работает как раньше.
pub fn discover_public_endpoint(sock: &UdpSocket) -> Option<SocketAddr> {
    for server in STUN_SERVERS {
        if let Some(addr) = stun_query_once(sock, server) {
            return Some(addr);
        }
    }
    None
}

fn stun_query_once(sock: &UdpSocket, server: &str) -> Option<SocketAddr> {
    let server_addr = server.to_socket_addrs().ok()?.find(|a| a.is_ipv4())?;

    let txn_id = random_transaction_id();
    let mut req = [0u8; 20];
    req[0..2].copy_from_slice(&STUN_BINDING_REQUEST.to_be_bytes());
    req[2..4].copy_from_slice(&0u16.to_be_bytes()); // length = 0, no attributes
    req[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    req[8..20].copy_from_slice(&txn_id);

    sock.send_to(&req, server_addr).ok()?;

    // Сохраняем и восстанавливаем исходный read_timeout — этот сокет живёт
    // дальше как EVRT-сессионный, нельзя оставить его в изменённом состоянии.
    let orig_timeout = sock.read_timeout().ok().flatten();
    let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
    let result = recv_stun_response(sock, &txn_id, Duration::from_millis(500));
    let _ = sock.set_read_timeout(orig_timeout);
    result
}

/// Читает ответы с таймаутом `budget` суммарно, игнорируя пакеты с чужим
/// transaction ID (могло прийти что-то ещё — маловероятно на свежем сокете,
/// но безопаснее не падать на первом же чужом пакете).
fn recv_stun_response(sock: &UdpSocket, txn_id: &[u8; 12], budget: Duration) -> Option<SocketAddr> {
    let deadline = std::time::Instant::now() + budget;
    let mut buf = [0u8; 512];
    loop {
        if std::time::Instant::now() >= deadline {
            return None;
        }
        let (n, _from) = sock.recv_from(&mut buf).ok()?;
        if n < 20 {
            continue;
        }
        let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
        if msg_type != STUN_BINDING_SUCCESS {
            continue;
        }
        if &buf[8..20] != txn_id {
            continue; // ответ на чужой запрос — ждём дальше в пределах budget
        }
        return parse_mapped_address(&buf[..n]);
    }
}

/// Парсит атрибуты STUN-ответа, ищет XOR-MAPPED-ADDRESS (предпочтительно,
/// RFC 5389) или MAPPED-ADDRESS (старый, RFC 3489) как fallback. IPv4 only.
fn parse_mapped_address(msg: &[u8]) -> Option<SocketAddr> {
    let length = u16::from_be_bytes([msg[2], msg[3]]) as usize;
    let end = (20 + length).min(msg.len());
    let mut pos = 20;
    let mut mapped: Option<SocketAddr> = None;
    let mut xor_mapped: Option<SocketAddr> = None;

    while pos + 4 <= end {
        let attr_type = u16::from_be_bytes([msg[pos], msg[pos + 1]]);
        let attr_len = u16::from_be_bytes([msg[pos + 2], msg[pos + 3]]) as usize;
        let val_start = pos + 4;
        let val_end = val_start + attr_len;
        if val_end > end || val_end < val_start {
            break;
        }
        let value = &msg[val_start..val_end];

        if attr_type == STUN_ATTR_XOR_MAPPED_ADDRESS && value.len() >= 8 && value[1] == 0x01 {
            let cookie = STUN_MAGIC_COOKIE.to_be_bytes();
            let port = u16::from_be_bytes([value[2], value[3]])
                ^ u16::from_be_bytes([cookie[0], cookie[1]]);
            let ip = Ipv4Addr::new(
                value[4] ^ cookie[0],
                value[5] ^ cookie[1],
                value[6] ^ cookie[2],
                value[7] ^ cookie[3],
            );
            xor_mapped = Some(SocketAddr::from((ip, port)));
        } else if attr_type == STUN_ATTR_MAPPED_ADDRESS && value.len() >= 8 && value[1] == 0x01 {
            let port = u16::from_be_bytes([value[2], value[3]]);
            let ip = Ipv4Addr::new(value[4], value[5], value[6], value[7]);
            mapped = Some(SocketAddr::from((ip, port)));
        }

        // Атрибуты паддятся до границы 4 байт.
        let padded_len = (attr_len + 3) & !3;
        pos = val_start + padded_len;
    }

    xor_mapped.or(mapped)
}

/// Простой xorshift64, засеянный текущим временем — этому не нужна
/// криптографическая случайность, только избежать коллизий transaction ID
/// между параллельными запросами. Без новых зависимостей.
fn random_transaction_id() -> [u8; 12] {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x1234_5678_9abc_def0)
        ^ 0x9E37_79B9_7F4A_7C15;
    let mut state = seed | 1; // xorshift needs a non-zero state
    let mut out = [0u8; 12];
    for chunk in out.chunks_mut(8) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let bytes = state.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_some_address() {
        // На любой машине с сетью должен быть хотя бы один не-loopback IPv4
        let addrs = local_ipv4_addresses();
        // Не паникует, возвращает Vec (может быть пустым на изолированной CI)
        for ip in &addrs {
            assert!(!ip.is_loopback());
        }
    }

    #[test]
    fn candidate_string_format() {
        let s = candidate_endpoints(45123);
        // Либо пусто, либо "ip:port,ip:port"
        if !s.is_empty() {
            for part in s.split(',') {
                assert!(part.contains(':'));
                assert!(part.ends_with(":45123"));
            }
        }
    }

    #[test]
    fn private_addresses_sorted_first() {
        let priv_ip = Ipv4Addr::new(192, 168, 1, 5);
        let pub_ip = Ipv4Addr::new(8, 8, 8, 8);
        let ll_ip = Ipv4Addr::new(169, 254, 1, 1);
        assert!(priority(&priv_ip) < priority(&pub_ip));
        assert!(priority(&pub_ip) < priority(&ll_ip));
    }

    #[test]
    fn parses_xor_mapped_address() {
        // Synthetic STUN Binding Success Response carrying XOR-MAPPED-ADDRESS
        // for 203.0.113.5:12345 — hand-built per RFC 5389 §15.2.
        let port: u16 = 12345;
        let ip = [203u8, 0, 113, 5];
        let cookie = STUN_MAGIC_COOKIE.to_be_bytes();
        let xport = port ^ u16::from_be_bytes([cookie[0], cookie[1]]);
        let xaddr = [
            ip[0] ^ cookie[0],
            ip[1] ^ cookie[1],
            ip[2] ^ cookie[2],
            ip[3] ^ cookie[3],
        ];

        let mut msg = Vec::new();
        msg.extend_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
        msg.extend_from_slice(&12u16.to_be_bytes()); // attribute length: 4 header + 8 value
        msg.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        msg.extend_from_slice(&[0xAB; 12]); // transaction id — irrelevant to this parser
        msg.extend_from_slice(&STUN_ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        msg.extend_from_slice(&8u16.to_be_bytes());
        msg.push(0x00); // reserved
        msg.push(0x01); // family = IPv4
        msg.extend_from_slice(&xport.to_be_bytes());
        msg.extend_from_slice(&xaddr);

        let got = parse_mapped_address(&msg).expect("should parse a mapped address");
        assert_eq!(
            got,
            SocketAddr::from((Ipv4Addr::new(203, 0, 113, 5), 12345))
        );
    }

    #[test]
    fn parses_legacy_mapped_address_as_fallback() {
        // Old-style MAPPED-ADDRESS (RFC 3489) — not XOR'd. Some STUN servers
        // send this instead of/alongside XOR-MAPPED-ADDRESS.
        let mut msg = Vec::new();
        msg.extend_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
        msg.extend_from_slice(&12u16.to_be_bytes());
        msg.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        msg.extend_from_slice(&[0xCD; 12]);
        msg.extend_from_slice(&STUN_ATTR_MAPPED_ADDRESS.to_be_bytes());
        msg.extend_from_slice(&8u16.to_be_bytes());
        msg.push(0x00);
        msg.push(0x01);
        msg.extend_from_slice(&9999u16.to_be_bytes());
        msg.extend_from_slice(&[198, 51, 100, 7]);

        let got = parse_mapped_address(&msg).expect("should parse legacy mapped address");
        assert_eq!(
            got,
            SocketAddr::from((Ipv4Addr::new(198, 51, 100, 7), 9999))
        );
    }

    #[test]
    fn parse_mapped_address_rejects_truncated_message() {
        // Header claims an attribute that doesn't fit in the buffer — must
        // not panic (index out of bounds) or produce a bogus address.
        let mut msg = Vec::new();
        msg.extend_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
        msg.extend_from_slice(&100u16.to_be_bytes()); // claims 100 bytes of attrs
        msg.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        msg.extend_from_slice(&[0; 12]);
        // ...but the buffer ends here, no attribute bytes actually present.
        assert_eq!(parse_mapped_address(&msg), None);
    }

    #[test]
    fn transaction_ids_are_twelve_bytes_and_vary() {
        let a = random_transaction_id();
        let b = random_transaction_id();
        assert_eq!(a.len(), 12);
        // Not a strict guarantee (could coincide), but overwhelmingly likely
        // to differ across two calls — catches an accidentally-constant seed.
        assert_ne!(a, b);
    }

    #[test]
    #[ignore] // needs real internet access to a public STUN server; run explicitly
    fn discover_public_endpoint_resolves_against_a_real_stun_server() {
        // Live network test (ROADMAP.md Phase 5.1) — same call
        // start_host_experiment makes on the real EVRT2 UDP socket. Proves the
        // whole path end to end: send Binding Request, parse the response,
        // return a routable public SocketAddr — not just the parser unit
        // tests above, which never touch the network.
        let sock = UdpSocket::bind("0.0.0.0:0").expect("bind ephemeral UDP socket");
        let addr = discover_public_endpoint(&sock).expect(
            "expected a public endpoint from stun.l.google.com / stun.cloudflare.com / stun1.l.google.com",
        );
        assert!(!addr.ip().is_unspecified());
        assert!(addr.port() > 0);
        println!("discovered public endpoint: {addr}");
    }
}
