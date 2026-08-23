//! The V2 packet: a framed message wrapped in a 40-byte header, ECB-encrypted,
//! with an MD5 tail.
//!
//! ```text
//! 5A 5A | 01 11 | len(LE16) | 20 00 | msg id (4) | timestamp (8) |
//! device id (LE64) | 12 reserved | AES-ECB(frame) | MD5(all of the above + SIGN_KEY)
//! ```
//!
//! V2 devices speak this directly over TCP. V3 devices carry the very same
//! packet inside an encrypted session — see [`crate::lan`].

use crate::security::{self, CryptoError};

pub const HEADER_LEN: usize = 40;
pub const TAIL_LEN: usize = 16;

#[derive(Debug, PartialEq, Eq)]
pub enum PacketError {
    TooShort(usize),
    NotAPacket([u8; 2]),
    Truncated {
        declared: usize,
        actual: usize,
    },
    /// The MD5 tail did not match, so the packet was corrupted or forged.
    BadDigest,
    Crypto(CryptoError),
}

impl From<CryptoError> for PacketError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}

impl core::fmt::Display for PacketError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooShort(n) => write!(f, "packet is only {n} bytes"),
            Self::NotAPacket(b) => {
                write!(f, "packet starts 0x{:02X}{:02X}, not 0x5A5A", b[0], b[1])
            }
            Self::Truncated { declared, actual } => {
                write!(f, "packet declares {declared} bytes but {actual} arrived")
            }
            Self::BadDigest => write!(f, "packet MD5 digest does not match"),
            Self::Crypto(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PacketError {}

/// The 8-byte timestamp field: two decimal digits per byte, most significant
/// last — `[centiseconds, s, m, H, D, M, YY, YYYY/100]`.
///
/// Nothing appears to validate it, but it is part of the format, so it is built
/// properly rather than zero-filled.
pub fn timestamp_from_unix(secs: u64, centis: u8) -> [u8; 8] {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (hour, min, sec) = (
        (rem / 3600) as u8,
        ((rem % 3600) / 60) as u8,
        (rem % 60) as u8,
    );
    // Civil date from a day count — Howard Hinnant's algorithm, so this needs no
    // date crate and stays correct past 2100.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let year = (if month <= 2 { y + 1 } else { y }) as u32;
    [
        centis,
        sec,
        min,
        hour,
        day,
        month,
        (year % 100) as u8,
        (year / 100) as u8,
    ]
}

fn now_timestamp() -> [u8; 8] {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => timestamp_from_unix(d.as_secs(), (d.subsec_millis() / 10) as u8),
        // A clock before the epoch is not worth failing a request over.
        Err(_) => [0; 8],
    }
}

/// Wrap a framed message in a V2 packet addressed to `device_id`.
pub fn encode(device_id: u64, frame: &[u8]) -> Vec<u8> {
    encode_at(device_id, frame, now_timestamp())
}

/// [`encode`] with the timestamp supplied, so tests are deterministic.
pub fn encode_at(device_id: u64, frame: &[u8], ts: [u8; 8]) -> Vec<u8> {
    let body = security::encrypt_aes(frame);
    let length = (HEADER_LEN + body.len() + TAIL_LEN) as u16;
    let mut p = Vec::with_capacity(length as usize);
    p.extend_from_slice(&[0x5A, 0x5A, 0x01, 0x11]);
    p.extend_from_slice(&length.to_le_bytes());
    p.extend_from_slice(&[0x20, 0x00]);
    p.extend_from_slice(&[0u8; 4]); // message id, unused by the devices we drive
    p.extend_from_slice(&ts);
    p.extend_from_slice(&device_id.to_le_bytes());
    p.extend_from_slice(&[0u8; 12]);
    p.extend_from_slice(&body);
    let digest = security::sign(&p);
    p.extend_from_slice(&digest);
    debug_assert_eq!(p.len(), length as usize);
    p
}

/// Verify a V2 packet and return the framed message inside it.
pub fn decode(packet: &[u8]) -> Result<Vec<u8>, PacketError> {
    if packet.len() < HEADER_LEN + TAIL_LEN {
        return Err(PacketError::TooShort(packet.len()));
    }
    if packet[..2] != [0x5A, 0x5A] {
        return Err(PacketError::NotAPacket([packet[0], packet[1]]));
    }
    let declared = u16::from_le_bytes([packet[4], packet[5]]) as usize;
    if packet.len() < declared || declared < HEADER_LEN + TAIL_LEN {
        return Err(PacketError::Truncated {
            declared,
            actual: packet.len(),
        });
    }
    let packet = &packet[..declared];
    let split = packet.len() - TAIL_LEN;
    if security::sign(&packet[..split]) != packet[split..] {
        return Err(PacketError::BadDigest);
    }
    Ok(security::decrypt_aes(&packet[HEADER_LEN..split])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{self, DeviceType, FrameType};

    #[test]
    fn round_trips_a_frame() {
        let inner = frame::build(
            DeviceType::AirConditioner,
            FrameType::Query,
            &[1, 2, 3, 4, 5],
        );
        let packet = encode(153_931_628_470_980, &inner);
        assert_eq!(&packet[..2], &[0x5A, 0x5A]);
        assert_eq!(
            u16::from_le_bytes([packet[4], packet[5]]) as usize,
            packet.len()
        );
        assert_eq!(decode(&packet).unwrap(), inner);
    }

    #[test]
    fn device_id_is_little_endian_in_the_header() {
        let id = 0x0102_0304_0506_0708u64;
        let packet = encode(id, &[0xAA, 11, 0xAC, 0, 0, 0, 0, 0, 0, 3, 0]);
        assert_eq!(&packet[20..28], &id.to_le_bytes());
    }

    #[test]
    fn a_tampered_packet_fails_its_digest() {
        let inner = frame::build(DeviceType::AirConditioner, FrameType::Query, &[7; 8]);
        let mut packet = encode(1, &inner);
        packet[HEADER_LEN] ^= 0x01;
        assert_eq!(decode(&packet), Err(PacketError::BadDigest));
    }

    #[test]
    fn junk_is_rejected_by_shape() {
        assert!(matches!(decode(&[0u8; 10]), Err(PacketError::TooShort(10))));
        let mut p = vec![0u8; 60];
        p[0] = 0xAA;
        assert!(matches!(decode(&p), Err(PacketError::NotAPacket(_))));
    }

    #[test]
    fn timestamp_encodes_digits_per_byte() {
        // 2026-08-23T12:34:56Z
        let ts = timestamp_from_unix(1_787_488_496, 7);
        assert_eq!(ts[0], 7, "centiseconds");
        assert_eq!(ts[1], 56, "seconds");
        assert_eq!(ts[2], 34, "minutes");
        assert_eq!(ts[3], 12, "hours");
        assert_eq!(ts[4], 23, "day");
        assert_eq!(ts[5], 8, "month");
        assert_eq!(ts[6], 26, "year within century");
        assert_eq!(ts[7], 20, "century");
    }
}
