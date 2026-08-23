//! Is this request from the LAN?
//!
//! Enrolment *approval* is an admin action from the trusted network: the API key
//! alone must not be enough to pair a device, because a key can leak while
//! standing in someone's flat cannot.
//!
//! Behind a reverse proxy the socket peer is the proxy — usually 127.0.0.1 — and
//! the real client is in `X-Forwarded-For`. That header is only trusted when the
//! deployment says it is behind a proxy, because otherwise any client could send
//! it and claim to be local.

use std::net::IpAddr;

/// The best available client address.
///
/// `behind_proxy` must reflect the deployment. Trusting `X-Forwarded-For`
/// unconditionally would let anyone on the internet assert a private address and
/// approve their own pairing.
pub fn client_ip(
    peer: Option<IpAddr>,
    forwarded_for: Option<&str>,
    behind_proxy: bool,
) -> Option<IpAddr> {
    if behind_proxy {
        if let Some(xff) = forwarded_for {
            // The left-most entry is the original client; the rest are proxies.
            if let Some(first) = xff.split(',').next() {
                if let Ok(ip) = first.trim().parse() {
                    return Some(ip);
                }
            }
        }
    }
    peer
}

/// True for "on my LAN or this machine" — loopback, RFC1918, link-local,
/// unique-local — rather than the public internet.
pub fn is_private_ip(ip: Option<IpAddr>) -> bool {
    match ip {
        Some(IpAddr::V4(v4)) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        Some(IpAddr::V6(v6)) => {
            // is_unique_local and is_unicast_link_local are unstable, so check
            // the prefixes directly: fc00::/7 and fe80::/10.
            let s = v6.segments();
            v6.is_loopback() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> Option<IpAddr> {
        Some(IpAddr::V4(Ipv4Addr::new(a, b, c, d)))
    }

    #[test]
    fn private_ranges_count_as_the_lan() {
        for ip in [
            v4(127, 0, 0, 1),
            v4(192, 168, 1, 73),
            v4(10, 0, 0, 5),
            v4(172, 16, 0, 1),
            v4(169, 254, 1, 1),
        ] {
            assert!(is_private_ip(ip), "{ip:?} should be private");
        }
    }

    #[test]
    fn public_addresses_do_not() {
        for ip in [v4(8, 8, 8, 8), v4(86, 33, 39, 199), v4(1, 1, 1, 1)] {
            assert!(!is_private_ip(ip), "{ip:?} must not be private");
        }
        assert!(!is_private_ip(None), "an unknown peer is not the LAN");
    }

    #[test]
    fn ipv6_loopback_and_local_prefixes_are_recognised() {
        assert!(is_private_ip(Some(IpAddr::V6(Ipv6Addr::LOCALHOST))));
        // fd00::/8 within fc00::/7
        assert!(is_private_ip(Some(IpAddr::V6(Ipv6Addr::new(
            0xfd00, 0, 0, 0, 0, 0, 0, 1
        )))));
        // fe80::/10
        assert!(is_private_ip(Some(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        )))));
        // A public 2000::/3 address
        assert!(!is_private_ip(Some(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0x4860, 0, 0, 0, 0, 0, 1
        )))));
    }

    #[test]
    fn forwarded_for_is_ignored_unless_behind_a_proxy() {
        // The attack this prevents: anyone claiming to be on the LAN.
        let peer = v4(8, 8, 8, 8);
        let spoofed = Some("192.168.1.50");
        assert_eq!(
            client_ip(peer, spoofed, false),
            peer,
            "must not trust the header"
        );
        assert!(!is_private_ip(client_ip(peer, spoofed, false)));
    }

    #[test]
    fn forwarded_for_is_honoured_when_configured() {
        let proxy = v4(127, 0, 0, 1);
        let real = client_ip(proxy, Some("192.168.1.50"), true);
        assert_eq!(real, v4(192, 168, 1, 50));
    }

    #[test]
    fn the_leftmost_forwarded_entry_is_the_client() {
        let real = client_ip(
            v4(127, 0, 0, 1),
            Some("192.168.1.50, 10.0.0.1, 172.16.0.1"),
            true,
        );
        assert_eq!(real, v4(192, 168, 1, 50));
    }

    #[test]
    fn a_malformed_forwarded_header_falls_back_to_the_peer() {
        let peer = v4(127, 0, 0, 1);
        assert_eq!(client_ip(peer, Some("not-an-ip"), true), peer);
        assert_eq!(client_ip(peer, Some(""), true), peer);
    }
}
