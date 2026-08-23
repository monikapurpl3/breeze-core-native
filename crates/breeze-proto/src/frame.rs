//! The innermost layer: 0xAA-framed appliance messages.
//!
//! This is what actually carries a command or a state report. Everything else —
//! the V2 packet, the V3 session — is transport wrapped around one of these.
//!
//! ```text
//! 0xAA  len  type  ..  ..  ..  ..  ..  ver  frame_type  <payload...>  checksum
//!  0     1    2                            8      9         10..        -1
//! ```
//!
//! `len` counts the 10-byte header plus the payload but *not* the checksum byte,
//! which is why a frame is `len + 1` bytes long.

pub const HEADER_LEN: usize = 10;

/// Appliance type. Only air conditioners are in scope here; the commercial
/// variant exists in the wild and is explicitly not supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DeviceType {
    AirConditioner = 0xAC,
}

/// What a frame is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    Control = 0x02,
    Query = 0x03,
    Report = 0x04,
    AbnormalReport = 0x06,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Shorter than a header, so nothing can be read from it.
    TooShort(usize),
    /// `len` disagrees with how many bytes actually arrived.
    LengthMismatch { declared: usize, actual: usize },
    /// Trailing checksum did not match the body.
    BadChecksum { expected: u8, found: u8 },
    /// Frame is for an appliance type we do not speak.
    WrongDeviceType(u8),
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooShort(n) => write!(
                f,
                "frame is {n} bytes, shorter than the {HEADER_LEN}-byte header"
            ),
            Self::LengthMismatch { declared, actual } => {
                write!(f, "frame declares {declared} bytes but {actual} arrived")
            }
            Self::BadChecksum { expected, found } => {
                write!(
                    f,
                    "frame checksum is 0x{found:02X}, expected 0x{expected:02X}"
                )
            }
            Self::WrongDeviceType(t) => write!(
                f,
                "frame is for device type 0x{t:02X}, not an air conditioner"
            ),
        }
    }
}

impl std::error::Error for FrameError {}

/// Two's complement of the byte sum — the frame's trailing checksum.
///
/// Note this is *not* the CRC-8 in [`crate::crc8`]. A frame carries both: the
/// CRC covers the command payload, this covers the framed message.
pub fn checksum(data: &[u8]) -> u8 {
    let sum = data.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    (!sum).wrapping_add(1)
}

/// Wrap `payload` in a frame for the given device and frame type.
pub fn build(device: DeviceType, frame_type: FrameType, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0u8; HEADER_LEN];
    frame[0] = 0xAA;
    frame[1] = (payload.len() + HEADER_LEN) as u8;
    frame[2] = device as u8;
    // frame[8] is the device's protocol version, always 0 for what we send.
    frame[9] = frame_type as u8;
    frame.extend_from_slice(payload);
    let c = checksum(&frame[1..]);
    frame.push(c);
    frame
}

/// Check a received frame and return its payload (everything between the header
/// and the checksum byte).
pub fn parse(frame: &[u8], expected: DeviceType) -> Result<&[u8], FrameError> {
    if frame.len() < HEADER_LEN {
        return Err(FrameError::TooShort(frame.len()));
    }
    // Some units send a frame longer than `len` claims; msmart tolerates that and
    // so must we, but a frame *shorter* than declared is truncated and unusable.
    let declared = frame[1] as usize + 1;
    if frame.len() < declared {
        return Err(FrameError::LengthMismatch {
            declared,
            actual: frame.len(),
        });
    }
    let frame = &frame[..declared];
    let expect = checksum(&frame[1..frame.len() - 1]);
    let found = frame[frame.len() - 1];
    if expect != found {
        return Err(FrameError::BadChecksum {
            expected: expect,
            found,
        });
    }
    if frame[2] != expected as u8 {
        return Err(FrameError::WrongDeviceType(frame[2]));
    }
    Ok(&frame[HEADER_LEN..frame.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_then_parse_round_trips() {
        let payload = [1u8, 2, 3, 4];
        let frame = build(DeviceType::AirConditioner, FrameType::Query, &payload);
        assert_eq!(frame[0], 0xAA);
        assert_eq!(frame[1] as usize, payload.len() + HEADER_LEN);
        assert_eq!(frame[2], 0xAC);
        assert_eq!(frame[9], 0x03);
        assert_eq!(frame.len(), payload.len() + HEADER_LEN + 1);
        assert_eq!(parse(&frame, DeviceType::AirConditioner).unwrap(), payload);
    }

    #[test]
    fn a_flipped_bit_is_caught() {
        let mut frame = build(DeviceType::AirConditioner, FrameType::Query, &[9, 9, 9]);
        frame[11] ^= 0x01;
        assert!(matches!(
            parse(&frame, DeviceType::AirConditioner),
            Err(FrameError::BadChecksum { .. })
        ));
    }

    #[test]
    fn short_and_truncated_frames_are_rejected() {
        assert_eq!(
            parse(&[0xAA, 2], DeviceType::AirConditioner),
            Err(FrameError::TooShort(2))
        );
        let frame = build(DeviceType::AirConditioner, FrameType::Query, &[1, 2, 3]);
        let cut = &frame[..frame.len() - 2];
        assert!(matches!(
            parse(cut, DeviceType::AirConditioner),
            Err(FrameError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn the_commercial_appliance_type_is_refused() {
        let mut frame = build(DeviceType::AirConditioner, FrameType::Query, &[0]);
        frame[2] = 0xCC;
        let c = checksum(&frame[1..frame.len() - 1]);
        let last = frame.len() - 1;
        frame[last] = c;
        assert_eq!(
            parse(&frame, DeviceType::AirConditioner),
            Err(FrameError::WrongDeviceType(0xCC))
        );
    }
}
