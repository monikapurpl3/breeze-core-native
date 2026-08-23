//! Finding units on the LAN.
//!
//! A fixed 72-byte hello goes out by broadcast; anything that answers identifies
//! itself in an ECB-encrypted blob. The reply's first two bytes say which
//! protocol generation you are talking to.
//!
//! The parsing here is pure — the socket work lives in the caller — so it can be
//! tested against captured replies without a network.

use crate::security;
use std::net::Ipv4Addr;

/// The vendor's discovery hello. Opaque, and reproduced exactly.
pub const DISCOVERY_MSG: [u8; 72] = [
    0x5a, 0x5a, 0x01, 0x11, 0x48, 0x00, 0x92, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7f, 0x75, 0xbd, 0x6b, 0x3e, 0x4f, 0x8b, 0x76,
    0x2e, 0x84, 0x9c, 0x6e, 0x57, 0x8d, 0x65, 0x90, 0x03, 0x6e, 0x9d, 0x43, 0x42, 0xa5, 0x0f, 0x1f,
    0x56, 0x9e, 0xb8, 0xec, 0x91, 0x8e, 0x92, 0xe5,
];

/// Units listen on both of these; asking twice costs nothing and some firmware
/// only answers on one.
pub const DISCOVERY_PORTS: [u16; 2] = [6445, 20086];

/// Which generation of the protocol a unit speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    /// XML over TCP. Needs a follow-up query and is not supported.
    V1,
    /// Plain packets.
    V2,
    /// Packets inside an authenticated session. Needs a token and key.
    V3,
}

/// What a unit told us about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The address we actually received from, which is authoritative — a unit
    /// behind NAT or with a stale lease can report a different one.
    pub ip: Ipv4Addr,
    /// The address the unit *claims*, kept because a mismatch is worth logging.
    pub reported_ip: Ipv4Addr,
    pub port: u16,
    pub id: u64,
    pub serial_number: String,
    pub ssid: String,
    pub device_type: u8,
    pub version: Version,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DiscoverError {
    /// Header matched nothing we know.
    UnknownVersion([u8; 2]),
    /// V1 units need a TCP follow-up we do not implement.
    V1Unsupported,
    /// Reply was too short for the fields it must contain.
    Truncated(usize),
    /// Decryption or padding failed.
    Undecryptable,
    /// The SSID did not carry a parseable `..._<type>_...` device type.
    NoDeviceType(String),
}

impl core::fmt::Display for DiscoverError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownVersion(b) => write!(
                f,
                "reply starts 0x{:02X}{:02X}, no known version",
                b[0], b[1]
            ),
            Self::V1Unsupported => write!(f, "V1 device: XML discovery is not supported"),
            Self::Truncated(n) => write!(f, "reply of {n} bytes is too short"),
            Self::Undecryptable => write!(f, "reply payload would not decrypt"),
            Self::NoDeviceType(s) => write!(f, "cannot read a device type from SSID {s:?}"),
        }
    }
}

impl std::error::Error for DiscoverError {}

/// Identify the protocol generation from a reply's first bytes.
pub fn version_of(reply: &[u8]) -> Result<Version, DiscoverError> {
    match reply {
        [0x5a, 0x5a, ..] => Ok(Version::V2),
        [0x83, 0x70, ..] => Ok(Version::V3),
        // V1 answers with XML rather than a binary header.
        [b'<', ..] => Ok(Version::V1),
        [a, b, ..] => Err(DiscoverError::UnknownVersion([*a, *b])),
        _ => Err(DiscoverError::Truncated(reply.len())),
    }
}

/// Parse a discovery reply received from `src`.
pub fn parse(src: Ipv4Addr, reply: &[u8]) -> Result<Discovered, DiscoverError> {
    let version = version_of(reply)?;
    if version == Version::V1 {
        return Err(DiscoverError::V1Unsupported);
    }

    // V3 wraps the V2 reply in an 8-byte header and a 16-byte trailing hash.
    let body: &[u8] = if version == Version::V3 {
        reply
            .get(
                8..reply
                    .len()
                    .checked_sub(16)
                    .ok_or(DiscoverError::Truncated(reply.len()))?,
            )
            .ok_or(DiscoverError::Truncated(reply.len()))?
    } else {
        reply
    };

    let end = body
        .len()
        .checked_sub(16)
        .ok_or(DiscoverError::Truncated(reply.len()))?;
    let encrypted = body
        .get(40..end)
        .ok_or(DiscoverError::Truncated(reply.len()))?;
    let id_bytes = body
        .get(20..26)
        .ok_or(DiscoverError::Truncated(reply.len()))?;

    // The id is a 48-bit little-endian field.
    let mut id = 0u64;
    for (i, b) in id_bytes.iter().enumerate() {
        id |= (*b as u64) << (8 * i);
    }

    let plain = security::decrypt_aes(encrypted).map_err(|_| DiscoverError::Undecryptable)?;
    if plain.len() < 41 {
        return Err(DiscoverError::Truncated(plain.len()));
    }
    // The address is stored most-significant-octet last.
    let reported_ip = Ipv4Addr::new(plain[3], plain[2], plain[1], plain[0]);
    let port = u16::from_le_bytes([plain[4], plain[5]]);
    let serial_number = String::from_utf8_lossy(&plain[8..40]).into_owned();
    let name_len = plain[40] as usize;
    let ssid = String::from_utf8_lossy(
        plain
            .get(41..41 + name_len)
            .ok_or(DiscoverError::Truncated(plain.len()))?,
    )
    .into_owned();

    // SSIDs look like `net_ac_19CA`: the middle field is the appliance type in hex.
    let device_type = ssid
        .split('_')
        .nth(1)
        .and_then(|t| u8::from_str_radix(t, 16).ok())
        .ok_or_else(|| DiscoverError::NoDeviceType(ssid.clone()))?;

    Ok(Discovered {
        ip: src,
        reported_ip,
        port,
        id,
        serial_number,
        ssid,
        device_type,
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_come_from_the_header() {
        assert_eq!(version_of(&[0x5a, 0x5a, 0, 0]), Ok(Version::V2));
        assert_eq!(version_of(&[0x83, 0x70, 0, 0]), Ok(Version::V3));
        assert_eq!(version_of(b"<xml>"), Ok(Version::V1));
        assert_eq!(
            version_of(&[0x12, 0x34]),
            Err(DiscoverError::UnknownVersion([0x12, 0x34]))
        );
        assert_eq!(version_of(&[0x5a]), Err(DiscoverError::Truncated(1)));
    }

    #[test]
    fn v1_is_refused_rather_than_half_parsed() {
        assert_eq!(
            parse(Ipv4Addr::LOCALHOST, b"<?xml version=\"1.0\"?>"),
            Err(DiscoverError::V1Unsupported)
        );
    }

    #[test]
    fn a_truncated_reply_does_not_panic() {
        // Every length up to a plausible reply must fail cleanly, never index out
        // of bounds -- this parses data straight off the network.
        for n in 0..80 {
            let mut reply = vec![0u8; n];
            if n >= 2 {
                reply[0] = 0x5a;
                reply[1] = 0x5a;
            }
            let _ = parse(Ipv4Addr::LOCALHOST, &reply);
        }
        for n in 0..80 {
            let mut reply = vec![0u8; n];
            if n >= 2 {
                reply[0] = 0x83;
                reply[1] = 0x70;
            }
            let _ = parse(Ipv4Addr::LOCALHOST, &reply);
        }
    }

    /// Build a synthetic V2 reply so the happy path is covered without hardware.
    fn synthetic_reply(ip: Ipv4Addr, port: u16, id: u64, sn: &str, ssid: &str) -> Vec<u8> {
        let mut plain = Vec::new();
        let o = ip.octets();
        plain.extend_from_slice(&[o[3], o[2], o[1], o[0]]);
        plain.extend_from_slice(&port.to_le_bytes());
        plain.extend_from_slice(&[0, 0]);
        let mut sn32 = sn.as_bytes().to_vec();
        sn32.resize(32, b'0');
        plain.extend_from_slice(&sn32);
        plain.push(ssid.len() as u8);
        plain.extend_from_slice(ssid.as_bytes());

        let body = security::encrypt_aes(&plain);
        let mut reply = vec![0x5a, 0x5a];
        reply.resize(20, 0);
        reply.extend_from_slice(&id.to_le_bytes()[..6]);
        reply.resize(40, 0);
        reply.extend_from_slice(&body);
        reply.extend_from_slice(&[0u8; 16]); // trailing hash, unchecked here
        reply
    }

    #[test]
    fn parses_a_well_formed_reply() {
        let reply = synthetic_reply(
            Ipv4Addr::new(192, 168, 1, 73),
            6444,
            153_931_628_470_980,
            "000000P0000000Q1AC72DD0A23E40000",
            "net_ac_23E4",
        );
        let d = parse(Ipv4Addr::new(192, 168, 1, 73), &reply).unwrap();
        assert_eq!(d.reported_ip, Ipv4Addr::new(192, 168, 1, 73));
        assert_eq!(d.port, 6444);
        assert_eq!(d.id, 153_931_628_470_980);
        assert_eq!(d.ssid, "net_ac_23E4");
        assert_eq!(d.device_type, 0xAC);
        assert_eq!(d.version, Version::V2);
        assert_eq!(d.serial_number.len(), 32);
    }

    #[test]
    fn the_received_address_wins_over_the_claimed_one() {
        let reply = synthetic_reply(
            Ipv4Addr::new(10, 0, 0, 5), // what the unit thinks it is
            6444,
            1,
            "sn",
            "net_ac_00",
        );
        let d = parse(Ipv4Addr::new(192, 168, 1, 73), &reply).unwrap();
        assert_eq!(
            d.ip,
            Ipv4Addr::new(192, 168, 1, 73),
            "must trust the socket"
        );
        assert_eq!(
            d.reported_ip,
            Ipv4Addr::new(10, 0, 0, 5),
            "but keep the claim"
        );
    }

    #[test]
    fn an_ssid_without_a_type_field_is_an_error() {
        let reply = synthetic_reply(Ipv4Addr::LOCALHOST, 6444, 1, "sn", "nonsense");
        assert!(matches!(
            parse(Ipv4Addr::LOCALHOST, &reply),
            Err(DiscoverError::NoDeviceType(_))
        ));
    }

    #[test]
    fn the_hello_is_the_documented_length() {
        assert_eq!(DISCOVERY_MSG.len(), 72);
        assert_eq!(&DISCOVERY_MSG[..4], &[0x5a, 0x5a, 0x01, 0x11]);
    }
}
