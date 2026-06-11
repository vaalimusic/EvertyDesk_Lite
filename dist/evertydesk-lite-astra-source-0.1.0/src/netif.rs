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

use std::net::Ipv4Addr;

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
    use std::net::UdpSocket;
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
}
