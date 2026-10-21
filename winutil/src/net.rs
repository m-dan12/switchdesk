//! Сетевые адреса этого компьютера.

use std::net::Ipv4Addr;
use std::ptr;

use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

/// Адреса Tailscale лежат в 100.64.0.0/10.
pub fn is_tailscale(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (o[1] & 0xC0) == 0x40
}

/// Все IPv4-адреса всех интерфейсов.
pub fn local_ipv4() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    unsafe {
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size = 16 * 1024u32;
        // Список мог измениться между вызовами — повторяем при нехватке буфера.
        for _ in 0..4 {
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let head = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
            let rc = GetAdaptersAddresses(AF_INET as u32, flags, ptr::null(), head, &mut size);
            if rc == 111 {
                continue; // ERROR_BUFFER_OVERFLOW: size уже обновлён
            }
            if rc != 0 {
                return out;
            }
            let mut adapter = head;
            while !adapter.is_null() {
                let mut u = (*adapter).FirstUnicastAddress;
                while !u.is_null() {
                    let sa = (*u).Address.lpSockaddr as *const SOCKADDR_IN;
                    if !sa.is_null() && (*sa).sin_family == AF_INET {
                        let raw = (*sa).sin_addr.S_un.S_addr;
                        out.push(Ipv4Addr::from(u32::from_be(raw)));
                    }
                    u = (*u).Next;
                }
                adapter = (*adapter).Next;
            }
            break;
        }
    }
    out
}

/// Адрес этого компьютера в Tailscale, если он есть.
pub fn tailscale_ip() -> Option<Ipv4Addr> {
    local_ipv4().into_iter().find(|ip| is_tailscale(*ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailscale_range() {
        assert!(is_tailscale("100.64.0.1".parse().unwrap()));
        assert!(is_tailscale("100.127.255.254".parse().unwrap()));
        assert!(!is_tailscale("100.128.0.1".parse().unwrap()));
        assert!(!is_tailscale("100.63.0.1".parse().unwrap()));
        assert!(!is_tailscale("192.168.1.2".parse().unwrap()));
    }
}
