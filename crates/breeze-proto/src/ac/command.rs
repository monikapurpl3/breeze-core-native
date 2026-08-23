//! Commands we send to an air conditioner.
//!
//! A command is a payload, a rolling message id and a CRC, wrapped in a frame.
//! The message id is the caller's business: msmart increments a global, but a
//! device does not appear to care, and making it explicit keeps these functions
//! pure and therefore testable against fixed vectors.

use crate::ac::types::TemperatureType;
use crate::crc8;
use crate::frame::{self, DeviceType, FrameType};

/// Append the message id and CRC, then frame it.
fn finish(frame_type: FrameType, payload: &[u8], message_id: u8) -> Vec<u8> {
    let mut body = Vec::with_capacity(payload.len() + 2);
    body.extend_from_slice(payload);
    body.push(message_id);
    let crc = crc8::calculate(&body);
    body.push(crc);
    frame::build(DeviceType::AirConditioner, frame_type, &body)
}

/// Query basic state — power, mode, setpoint, fan, flap, temperatures.
///
/// The payload is mostly fixed bytes whose meaning is not documented anywhere;
/// they are reproduced exactly because the units expect them.
pub fn get_state(message_id: u8, temperature: TemperatureType) -> Vec<u8> {
    let payload = [
        0x41,
        0x81,
        0x00,
        0xFF,
        0x03,
        0xFF,
        0x00,
        temperature as u8,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x03,
    ];
    finish(FrameType::Query, &payload, message_id)
}

/// Query what the unit supports, so clients can hide controls the hardware lacks.
pub fn get_capabilities(message_id: u8) -> Vec<u8> {
    finish(FrameType::Query, &[0xB5, 0x01, 0x00], message_id)
}

/// The second half of the capability set. Some units answer only the first.
pub fn get_more_capabilities(message_id: u8) -> Vec<u8> {
    finish(FrameType::Query, &[0xB5, 0x01, 0x01, 0x01], message_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::HEADER_LEN;

    /// msmart's own framing test: with message id 0x11, the GetState payload is
    /// exactly these bytes. This is the vector that pins the whole command layer.
    #[test]
    fn get_state_matches_the_reference_bytes() {
        const EXPECTED: [u8; 23] = [
            0x41, 0x81, 0x00, 0xFF, 0x03, 0xFF, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x11, 0xF4,
        ];
        let f = get_state(0x11, TemperatureType::Indoor);
        assert_eq!(&f[HEADER_LEN..f.len() - 1], &EXPECTED[..]);
        assert_eq!(f[1] as usize, EXPECTED.len() + HEADER_LEN, "length byte");
        assert_eq!(f[2], DeviceType::AirConditioner as u8);
        assert_eq!(f[9], FrameType::Query as u8);
        // And the frame must validate under its own rules.
        assert!(frame::parse(&f, DeviceType::AirConditioner).is_ok());
    }

    #[test]
    fn the_message_id_lands_where_the_crc_covers_it() {
        // Two ids differ in exactly two bytes: the id and the CRC over it.
        let a = get_state(0x11, TemperatureType::Indoor);
        let b = get_state(0x12, TemperatureType::Indoor);
        let diff: Vec<usize> = a
            .iter()
            .zip(b.iter())
            .enumerate()
            .filter(|(_, (x, y))| x != y)
            .map(|(i, _)| i)
            .collect();
        // message id, its CRC, and the frame checksum that covers both.
        assert_eq!(diff.len(), 3, "differing byte positions: {diff:?}");
    }

    #[test]
    fn outdoor_query_differs_only_in_the_sensor_byte() {
        let indoor = get_state(1, TemperatureType::Indoor);
        let outdoor = get_state(1, TemperatureType::Outdoor);
        assert_eq!(indoor[HEADER_LEN + 7], 0x02);
        assert_eq!(outdoor[HEADER_LEN + 7], 0x03);
    }

    #[test]
    fn capability_queries_are_framed_as_queries() {
        for f in [get_capabilities(1), get_more_capabilities(1)] {
            assert_eq!(f[9], FrameType::Query as u8);
            assert_eq!(f[HEADER_LEN], 0xB5);
            assert!(frame::parse(&f, DeviceType::AirConditioner).is_ok());
        }
        // The "more" variant carries one extra selector byte.
        assert_eq!(
            get_more_capabilities(1).len(),
            get_capabilities(1).len() + 1
        );
    }
}
