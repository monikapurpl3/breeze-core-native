//! The crypto the Midea LAN protocol is built on.
//!
//! Deliberately boring: AES-ECB and AES-CBC, MD5, SHA-256 and XOR. No AEAD, no
//! exotic primitives — which is exactly why this port builds unchanged on every
//! architecture we care about, including the ones with no assembly backends.
//!
//! Two key sizes are in play and mixing them up costs an afternoon:
//!
//! * the **fixed** key (`ENC_KEY`, the MD5 of `SIGN_KEY`) is 16 bytes, so the
//!   packet-level ECB layer is AES-**128**;
//! * the **per-device** key from the cloud is 32 bytes, so the V3 session and
//!   its handshake are AES-**256**-CBC with an all-zero IV.

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use md5::Digest as _;

/// The vendor's fixed signing key. Public knowledge; it authenticates nothing
/// secret, it just makes the packet format self-checking.
pub const SIGN_KEY: &[u8] = b"xhdiwjnchekd4d512chdjx5d8e4c394D2D7S";

/// Errors from the crypto layer. Deliberately coarse: a caller can only ever
/// react by dropping the packet.
#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// Input was not a whole number of AES blocks.
    Misaligned(usize),
    /// PKCS#7 padding byte was outside 1..=16, or longer than the plaintext.
    BadPadding(usize),
}

impl core::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Misaligned(n) => {
                write!(f, "{n} bytes is not a multiple of the 16-byte AES block")
            }
            Self::BadPadding(p) => write!(f, "invalid PKCS#7 padding byte {p}"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// The 16-byte key for the packet-level ECB layer.
pub fn enc_key() -> [u8; 16] {
    md5::Md5::digest(SIGN_KEY).into()
}

/// MD5 of `data` with the signing key appended — the trailing 16 bytes of a V2
/// packet.
pub fn sign(data: &[u8]) -> [u8; 16] {
    let mut h = md5::Md5::new();
    h.update(data);
    h.update(SIGN_KEY);
    h.finalize().into()
}

/// The cloud's "udpid" for a device: SHA-256 of the id, folded in half by XOR.
///
/// Both endiannesses of the id are tried during pairing, because which one a
/// given unit was registered under is not knowable in advance.
pub fn udpid(device_id: &[u8]) -> [u8; 16] {
    let h = sha2::Sha256::digest(device_id);
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = h[i] ^ h[i + 16];
    }
    out
}

/// AES-128-ECB encrypt with PKCS#7 padding, under the fixed key.
pub fn encrypt_aes(data: &[u8]) -> Vec<u8> {
    let cipher = aes::Aes128::new(&GenericArray::from(enc_key()));
    let pad = 16 - (data.len() % 16);
    let mut buf = data.to_vec();
    buf.extend(std::iter::repeat_n(pad as u8, pad));
    for block in buf.chunks_mut(16) {
        let mut b = GenericArray::clone_from_slice(block);
        cipher.encrypt_block(&mut b);
        block.copy_from_slice(&b);
    }
    buf
}

/// AES-128-ECB decrypt and strip PKCS#7 padding, under the fixed key.
pub fn decrypt_aes(data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err(CryptoError::Misaligned(data.len()));
    }
    let cipher = aes::Aes128::new(&GenericArray::from(enc_key()));
    let mut out = data.to_vec();
    for block in out.chunks_mut(16) {
        let mut b = GenericArray::clone_from_slice(block);
        cipher.decrypt_block(&mut b);
        block.copy_from_slice(&b);
    }
    let pad = *out.last().expect("non-empty by the check above") as usize;
    if pad == 0 || pad > 16 || pad > out.len() {
        return Err(CryptoError::BadPadding(pad));
    }
    out.truncate(out.len() - pad);
    Ok(out)
}

/// AES-256-CBC encrypt, zero IV, no padding. Written out rather than pulled in
/// from a mode crate: every payload here is already block-aligned by
/// construction, and this stays short enough to audit against the reference.
pub fn encrypt_aes_cbc(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if !data.len().is_multiple_of(16) {
        return Err(CryptoError::Misaligned(data.len()));
    }
    let cipher = aes::Aes256::new(&GenericArray::from(*key));
    let mut prev = [0u8; 16];
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(16) {
        let mut block = [0u8; 16];
        for i in 0..16 {
            block[i] = chunk[i] ^ prev[i];
        }
        let mut b = GenericArray::from(block);
        cipher.encrypt_block(&mut b);
        prev.copy_from_slice(&b);
        out.extend_from_slice(&b);
    }
    Ok(out)
}

/// AES-256-CBC decrypt, zero IV, no padding.
pub fn decrypt_aes_cbc(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if !data.len().is_multiple_of(16) {
        return Err(CryptoError::Misaligned(data.len()));
    }
    let cipher = aes::Aes256::new(&GenericArray::from(*key));
    let mut prev = [0u8; 16];
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(16) {
        let mut b = GenericArray::clone_from_slice(chunk);
        cipher.decrypt_block(&mut b);
        for i in 0..16 {
            out.push(b[i] ^ prev[i]);
        }
        prev.copy_from_slice(chunk);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enc_key_is_the_md5_of_the_sign_key() {
        // Pinned so a future refactor of the key derivation is caught here rather
        // than by three air conditioners going silent.
        assert_eq!(
            enc_key(),
            [
                0x6a, 0x92, 0xef, 0x40, 0x6b, 0xad, 0x2f, 0x03, 0x59, 0xba, 0xad, 0x99, 0x41, 0x71,
                0xea, 0x6d
            ]
        );
    }

    #[test]
    fn ecb_round_trips_and_pads() {
        for len in [1usize, 15, 16, 17, 40] {
            let plain: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let ct = encrypt_aes(&plain);
            assert_eq!(ct.len() % 16, 0, "ciphertext must be block aligned");
            assert!(
                ct.len() > plain.len(),
                "PKCS#7 always adds at least one byte"
            );
            assert_eq!(decrypt_aes(&ct).unwrap(), plain);
        }
    }

    #[test]
    fn ecb_rejects_misaligned_and_bad_padding() {
        assert_eq!(decrypt_aes(&[0u8; 7]), Err(CryptoError::Misaligned(7)));
        assert_eq!(decrypt_aes(&[]), Err(CryptoError::Misaligned(0)));
        // A block that decrypts to something with a nonsense final byte.
        let mut ct = encrypt_aes(b"hello");
        ct[15] ^= 0xFF;
        assert!(matches!(decrypt_aes(&ct), Err(CryptoError::BadPadding(_))));
    }

    #[test]
    fn cbc_round_trips_with_a_32_byte_key() {
        let key = [7u8; 32];
        let plain = [0xABu8; 64];
        let ct = encrypt_aes_cbc(&key, &plain).unwrap();
        assert_ne!(ct[..16], ct[16..32], "CBC must not repeat blocks like ECB");
        assert_eq!(decrypt_aes_cbc(&key, &ct).unwrap(), plain);
    }

    #[test]
    fn cbc_rejects_misaligned_input() {
        let key = [0u8; 32];
        assert_eq!(
            encrypt_aes_cbc(&key, &[0u8; 3]),
            Err(CryptoError::Misaligned(3))
        );
    }

    #[test]
    fn udpid_folds_sha256_in_half() {
        let id = 153931628470980u64;
        let le = udpid(&id.to_le_bytes()[..6]);
        let be = udpid(&id.to_be_bytes()[2..]);
        assert_eq!(le.len(), 16);
        assert_ne!(
            le, be,
            "the two endiannesses must differ, or trying both is pointless"
        );
    }
}
