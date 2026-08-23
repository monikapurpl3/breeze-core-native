//! The V3 session: `0x8370` framing, the token handshake, and encrypted
//! request/response.
//!
//! ```text
//! 83 70 | len(BE16) | 20 | pad<<4 | type | <payload> | sign
//! ```
//!
//! `len` covers the payload, its padding and the signature, but *not* the 2-byte
//! packet id that precedes the payload — which is why the total is `len + 8`.
//!
//! Everything here is codec-only and takes bytes in and out. The socket lives in
//! the caller so this can be tested without one.

use crate::security::{self, CryptoError};
use sha2::Digest as _;

/// Packet types in the low nibble of the flags byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    HandshakeRequest = 0x0,
    HandshakeResponse = 0x1,
    EncryptedResponse = 0x3,
    EncryptedRequest = 0x6,
    Error = 0xF,
}

/// How long a session key is good for. The device stops honouring it eventually;
/// re-authenticating early is cheaper than discovering that mid-command.
pub const SESSION_LIFETIME_SECS: u64 = 12 * 60 * 60;

/// A unit is not ready the instant its handshake completes.
///
/// This is not defensive padding: without it, the first request after a
/// successful handshake gets no reply at all, and the failure looks exactly like
/// a network timeout. msmart has the same wait, uncommented beyond "sleep
/// briefly before requesting more data".
pub const POST_HANDSHAKE_SETTLE_MS: u64 = 1000;

#[derive(Debug, PartialEq, Eq)]
pub enum LanError {
    TooShort(usize),
    NotAPacket([u8; 2]),
    BadMagic(u8),
    Incomplete {
        need: usize,
        have: usize,
    },
    /// The device answered with an explicit error packet.
    DeviceError,
    UnexpectedType(u8),
    /// Signature over the plaintext did not match.
    BadSignature,
    /// The handshake payload was not the expected 64 bytes.
    BadHandshakeLength(usize),
    /// The key we hold does not match what the device expects.
    Rejected,
    Crypto(CryptoError),
}

impl From<CryptoError> for LanError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}

impl core::fmt::Display for LanError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooShort(n) => write!(f, "packet is only {n} bytes"),
            Self::NotAPacket(b) => {
                write!(f, "packet starts 0x{:02X}{:02X}, not 0x8370", b[0], b[1])
            }
            Self::BadMagic(m) => write!(f, "magic byte is 0x{m:02X}, not 0x20"),
            Self::Incomplete { need, have } => write!(f, "need {need} bytes, have {have}"),
            Self::DeviceError => write!(f, "device returned an error packet"),
            Self::UnexpectedType(t) => write!(f, "unexpected packet type {t}"),
            Self::BadSignature => write!(f, "packet signature does not match"),
            Self::BadHandshakeLength(n) => write!(f, "handshake payload is {n} bytes, expected 64"),
            Self::Rejected => write!(f, "device rejected the token and key"),
            Self::Crypto(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LanError {}

/// How many bytes the packet starting at `buf` will be, once complete.
///
/// Returns `None` until the 6-byte header has arrived. Callers reading from a
/// stream use this to know when to stop.
pub fn packet_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 6 {
        return None;
    }
    Some(u16::from_be_bytes([buf[2], buf[3]]) as usize + 8)
}

fn header(length: u16, flags: u8) -> [u8; 6] {
    let l = length.to_be_bytes();
    [0x83, 0x70, l[0], l[1], 0x20, flags]
}

/// Build the handshake request that opens a session: the cloud token, unencrypted.
pub fn encode_handshake(packet_id: u16, token: &[u8]) -> Vec<u8> {
    let mut pkt = header(token.len() as u16, PacketType::HandshakeRequest as u8).to_vec();
    pkt.extend_from_slice(&packet_id.to_be_bytes());
    pkt.extend_from_slice(token);
    pkt
}

/// Derive the session key from a handshake response.
///
/// The device returns 32 encrypted bytes plus their SHA-256. Decrypt under the
/// cloud key, check the digest, and XOR the two together — a mismatch means the
/// key is wrong, which is the only way to learn that.
pub fn derive_session_key(key: &[u8; 32], response: &[u8]) -> Result<[u8; 32], LanError> {
    if response.len() != 64 {
        return Err(LanError::BadHandshakeLength(response.len()));
    }
    let plain = security::decrypt_aes_cbc(key, &response[..32])?;
    if sha2::Sha256::digest(&plain)[..] != response[32..] {
        return Err(LanError::Rejected);
    }
    let mut session = [0u8; 32];
    for i in 0..32 {
        session[i] = plain[i] ^ key[i];
    }
    Ok(session)
}

/// Wrap `data` (a V2 packet) as an encrypted request.
///
/// `pad_filler` supplies the alignment bytes. Padding is discarded by the
/// receiver but covered by the signature, so it may be anything; taking it as a
/// parameter keeps this deterministic for tests.
pub fn encode_request(
    session_key: &[u8; 32],
    packet_id: u16,
    data: &[u8],
    pad_filler: u8,
) -> Result<Vec<u8>, LanError> {
    encode_encrypted(
        session_key,
        packet_id,
        data,
        pad_filler,
        PacketType::EncryptedRequest,
    )
}

/// Encrypted framing for either direction. Requests and responses differ only in
/// the type nibble -- but that nibble is inside the signed header, so a packet
/// cannot be re-typed after the fact. Tests that need to stand in for a device
/// build responses through here.
pub fn encode_encrypted(
    session_key: &[u8; 32],
    packet_id: u16,
    data: &[u8],
    pad_filler: u8,
    packet_type: PacketType,
) -> Result<Vec<u8>, LanError> {
    // The 2-byte packet id counts towards alignment, but not towards `len`.
    let remainder = (data.len() + 2) % 16;
    let pad = if remainder != 0 { 16 - remainder } else { 0 };
    let length = (data.len() + pad + 32) as u16;
    let hdr = header(length, ((pad as u8) << 4) | packet_type as u8);

    let mut payload = Vec::with_capacity(2 + data.len() + pad);
    payload.extend_from_slice(&packet_id.to_be_bytes());
    payload.extend_from_slice(data);
    payload.extend(std::iter::repeat_n(pad_filler, pad));

    let mut h = sha2::Sha256::new();
    h.update(hdr);
    h.update(&payload);
    let digest = h.finalize();

    let mut pkt = hdr.to_vec();
    pkt.extend_from_slice(&security::encrypt_aes_cbc(session_key, &payload)?);
    pkt.extend_from_slice(&digest);
    Ok(pkt)
}

/// Verify and decrypt an encrypted packet of **either** direction.
///
/// [`decode`] deliberately refuses to read a request as a response — a client
/// receiving its own request back is a bug worth surfacing. But the device side
/// of this protocol has to read requests, and so does anything standing in for a
/// device, so the shared half lives here.
pub fn decode_encrypted(
    packet: &[u8],
    session_key: &[u8; 32],
) -> Result<(PacketType, Vec<u8>), LanError> {
    if packet.len() < 6 + 32 {
        return Err(LanError::TooShort(packet.len()));
    }
    let flags = packet[5];
    let kind = match flags & 0xF {
        x if x == PacketType::EncryptedRequest as u8 => PacketType::EncryptedRequest,
        x if x == PacketType::EncryptedResponse as u8 => PacketType::EncryptedResponse,
        other => return Err(LanError::UnexpectedType(other)),
    };
    let (hdr, rest) = packet.split_at(6);
    let (body, rx_digest) = rest.split_at(rest.len() - 32);
    let plain = security::decrypt_aes_cbc(session_key, body)?;
    let mut h = sha2::Sha256::new();
    h.update(hdr);
    h.update(&plain);
    if h.finalize()[..] != *rx_digest {
        return Err(LanError::BadSignature);
    }
    let pad = (flags >> 4) as usize;
    if plain.len() < 2 + pad {
        return Err(LanError::TooShort(plain.len()));
    }
    Ok((kind, plain[2..plain.len() - pad].to_vec()))
}

/// The payload of a decoded packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    /// 64 bytes to feed to [`derive_session_key`].
    Handshake(Vec<u8>),
    /// A V2 packet.
    Response(Vec<u8>),
}

/// Verify and unwrap a received packet.
///
/// `session_key` is only needed for encrypted responses; a handshake reply can
/// be read before one exists.
pub fn decode(packet: &[u8], session_key: Option<&[u8; 32]>) -> Result<Decoded, LanError> {
    if packet.len() < 8 {
        return Err(LanError::TooShort(packet.len()));
    }
    if packet[..2] != [0x83, 0x70] {
        return Err(LanError::NotAPacket([packet[0], packet[1]]));
    }
    if packet[4] != 0x20 {
        return Err(LanError::BadMagic(packet[4]));
    }
    let need = packet_len(packet).expect("length checked above");
    if packet.len() < need {
        return Err(LanError::Incomplete {
            need,
            have: packet.len(),
        });
    }
    let packet = &packet[..need];
    let flags = packet[5];

    match flags & 0xF {
        x if x == PacketType::Error as u8 => Err(LanError::DeviceError),
        x if x == PacketType::HandshakeResponse as u8 => {
            // Unencrypted: skip the header and the packet id.
            Ok(Decoded::Handshake(packet[8..].to_vec()))
        }
        x if x == PacketType::EncryptedResponse as u8 => {
            let key = session_key.ok_or(LanError::UnexpectedType(x))?;
            let (_, payload) = decode_encrypted(packet, key)?;
            Ok(Decoded::Response(payload))
        }
        other => Err(LanError::UnexpectedType(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [0x5A; 32];

    /// What a device would put on the wire for this payload.
    fn encode_response(key: &[u8; 32], id: u16, data: &[u8], filler: u8) -> Vec<u8> {
        encode_encrypted(key, id, data, filler, PacketType::EncryptedResponse)
            .expect("payload is block-aligned by construction")
    }

    #[test]
    fn a_request_round_trips_through_decode() {
        let session = [0x11u8; 32];
        for len in [1usize, 14, 30, 46, 100] {
            let data: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let pkt = encode_response(&session, 7, &data, 0);
            assert_eq!(
                packet_len(&pkt),
                Some(pkt.len()),
                "declared length must match"
            );
            assert_eq!(
                decode(&pkt, Some(&session)).unwrap(),
                Decoded::Response(data),
                "payload must survive padding of {len} bytes"
            );
        }
    }

    #[test]
    fn padding_is_never_visible_to_the_caller() {
        let session = [3u8; 32];
        // 13 bytes + 2 id = 15, so one byte of padding is added.
        let data = vec![0xAB; 13];
        let a = encode_response(&session, 1, &data, 0x00);
        let b = encode_response(&session, 1, &data, 0xFF);
        assert_ne!(a, b, "different filler must produce different ciphertext");
        assert_eq!(
            decode(&a, Some(&session)).unwrap(),
            decode(&b, Some(&session)).unwrap()
        );
    }

    #[test]
    fn a_tampered_response_fails_its_signature() {
        let session = [9u8; 32];
        let mut pkt = encode_response(&session, 1, &[1, 2, 3, 4], 0);
        pkt[10] ^= 0x01;
        assert_eq!(decode(&pkt, Some(&session)), Err(LanError::BadSignature));
    }

    #[test]
    fn the_wrong_session_key_does_not_decode() {
        let pkt = encode_response(&[1u8; 32], 1, &[1, 2, 3, 4], 0);
        assert_eq!(decode(&pkt, Some(&[2u8; 32])), Err(LanError::BadSignature));
    }

    #[test]
    fn handshake_request_is_the_token_in_the_clear() {
        let token = [0xEEu8; 64];
        let pkt = encode_handshake(0, &token);
        assert_eq!(&pkt[..2], &[0x83, 0x70]);
        assert_eq!(u16::from_be_bytes([pkt[2], pkt[3]]) as usize, token.len());
        assert_eq!(pkt[5] & 0xF, PacketType::HandshakeRequest as u8);
        assert_eq!(&pkt[8..], &token[..]);
    }

    #[test]
    fn session_key_derivation_detects_a_wrong_key() {
        // A response the device would only produce for a different key.
        let response = [0u8; 64];
        assert_eq!(derive_session_key(&KEY, &response), Err(LanError::Rejected));
    }

    #[test]
    fn session_key_derivation_checks_the_length() {
        assert_eq!(
            derive_session_key(&KEY, &[0u8; 32]),
            Err(LanError::BadHandshakeLength(32))
        );
    }

    #[test]
    fn a_valid_handshake_yields_key_xor_plaintext() {
        // Construct the response the device would send for a known plaintext.
        let plain = [0x77u8; 32];
        let encrypted = security::encrypt_aes_cbc(&KEY, &plain).unwrap();
        let mut response = encrypted;
        response.extend_from_slice(&sha2::Sha256::digest(plain));
        let session = derive_session_key(&KEY, &response).unwrap();
        for i in 0..32 {
            assert_eq!(session[i], plain[i] ^ KEY[i]);
        }
    }

    #[test]
    fn error_packets_are_surfaced_not_parsed() {
        let mut pkt = header(0, PacketType::Error as u8).to_vec();
        pkt.extend_from_slice(&[0, 0]);
        assert_eq!(decode(&pkt, None), Err(LanError::DeviceError));
    }

    #[test]
    fn malformed_packets_are_rejected_by_shape() {
        assert_eq!(decode(&[0x83], None), Err(LanError::TooShort(1)));
        assert_eq!(
            decode(&[0x12, 0x34, 0, 0, 0x20, 1, 0, 0], None),
            Err(LanError::NotAPacket([0x12, 0x34]))
        );
        assert_eq!(
            decode(&[0x83, 0x70, 0, 0, 0x99, 1, 0, 0], None),
            Err(LanError::BadMagic(0x99))
        );
    }

    #[test]
    fn a_partial_stream_reports_what_it_still_needs() {
        let pkt = encode_response(&[1u8; 32], 1, &[0; 20], 0);
        assert_eq!(packet_len(&pkt[..4]), None, "header not complete yet");
        let short = &pkt[..pkt.len() - 4];
        assert_eq!(
            decode(short, Some(&[1u8; 32])),
            Err(LanError::Incomplete {
                need: pkt.len(),
                have: short.len()
            })
        );
    }

    #[test]
    fn decode_encrypted_reads_both_directions() {
        let session = [0x21u8; 32];
        let data = [1u8, 2, 3, 4, 5];
        for kind in [PacketType::EncryptedRequest, PacketType::EncryptedResponse] {
            let pkt = encode_encrypted(&session, 3, &data, 0, kind).unwrap();
            let (got, payload) = decode_encrypted(&pkt, &session).unwrap();
            assert_eq!(got, kind);
            assert_eq!(payload, data);
        }
    }

    #[test]
    fn decode_still_refuses_a_request_even_though_decode_encrypted_accepts_one() {
        // The strict behaviour is the point: a client that reads its own request
        // back has a bug, and decode is what clients use.
        let session = [0x21u8; 32];
        let pkt = encode_encrypted(&session, 1, &[9; 8], 0, PacketType::EncryptedRequest).unwrap();
        assert!(decode_encrypted(&pkt, &session).is_ok());
        assert_eq!(
            decode(&pkt, Some(&session)),
            Err(LanError::UnexpectedType(PacketType::EncryptedRequest as u8))
        );
    }

    #[test]
    fn no_input_length_panics() {
        // This decodes bytes straight off a socket, so fuzz the shapes cheaply.
        for n in 0..64 {
            let mut pkt = vec![0u8; n];
            if n >= 2 {
                pkt[0] = 0x83;
                pkt[1] = 0x70;
            }
            if n >= 5 {
                pkt[4] = 0x20;
            }
            let _ = decode(&pkt, Some(&KEY));
        }
    }
}
