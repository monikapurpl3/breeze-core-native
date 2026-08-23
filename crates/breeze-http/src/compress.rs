//! Response compression.
//!
//! # gzip only, and why not brotli
//!
//! The reference offers brotli and gzip. This offers gzip alone, deliberately:
//!
//! * brotli's Rust implementation is several hundred kilobytes of binary for a
//!   few percent over gzip on documents this size — a poor trade in a project
//!   whose whole argument is fitting on a router;
//! * every HTTP client in play sends `gzip` in its `Accept-Encoding`, so nobody
//!   loses compression by its absence;
//! * brotli has already caused a real bug here. The reference's SSE route has to
//!   force `Content-Encoding: identity` because a brotli event-stream is garbage
//!   to a client that cannot decode it — Dart's `http` among them. Not shipping
//!   brotli removes that trap rather than working around it.
//!
//! # What is not compressed
//!
//! * **Anything already encoded.** The panel's assets are stored raw and
//!   compressed on the way out, but a reply that set its own `Content-Encoding`
//!   is left alone.
//! * **The SSE stream.** It never reaches this code — it writes its own headers
//!   and hijacks the socket — and it must not: a buffered compressor would hold
//!   events until its window filled, which is indistinguishable from a dead
//!   stream.
//! * **Small bodies.** Below [`MIN_SIZE`] the gzip header and trailer cost more
//!   than the saving, and a 200-byte state response is most of this API's
//!   traffic.
//! * **204 and 304.** They have no body, and a `Content-Encoding` on an empty
//!   response confuses caches.

use std::io::Write;

use flate2::write::GzEncoder;
use flate2::Compression;

use crate::respond::Reply;

/// Below this, compressing costs more than it saves.
///
/// A gzip stream carries an 18-byte header and trailer, and the deflate of a
/// short JSON object often exceeds the original. 512 is comfortably past the
/// point where a unit state or a program list starts to gain.
pub const MIN_SIZE: usize = 512;

/// Fast rather than best. Level 9 on a 133 KB panel file costs milliseconds of
/// CPU on a router for a few hundred bytes; level 1 gets most of the benefit for
/// almost none of the cost, and this is a LAN.
const LEVEL: Compression = Compression::new(1);

/// Whether the client said it can decode gzip.
///
/// Tolerant of the whole header grammar: `gzip`, `gzip;q=0.8`, a list, mixed
/// case, and `*`. A client that says nothing gets no compression — the safe
/// direction, since a body it cannot read is worse than a larger one.
pub fn accepts_gzip(accept_encoding: Option<&str>) -> bool {
    let Some(header) = accept_encoding else {
        return false;
    };
    for part in header.split(',') {
        let mut bits = part.split(';');
        let coding = bits.next().unwrap_or("").trim();
        // `q=0` is an explicit refusal, and it is how a client opts out of an
        // encoding the wildcard would otherwise have accepted for it.
        let refused = bits.any(|p| {
            let p = p.trim().replace(' ', "");
            p == "q=0" || p == "q=0.0" || p == "q=0.00"
        });
        if refused {
            continue;
        }
        if coding.eq_ignore_ascii_case("gzip") || coding == "*" {
            return true;
        }
    }
    false
}

/// Compress a reply in place, if it is worth it and the client can take it.
///
/// Returns whether it did, which is only used by tests and diagnostics — the
/// caller does not need to care.
pub fn maybe_compress(reply: &mut Reply, accept_encoding: Option<&str>) -> bool {
    if !accepts_gzip(accept_encoding) {
        return false;
    }
    if reply.body.len() < MIN_SIZE {
        return false;
    }
    // 204/304 have no body; 1xx likewise.
    if reply.status == 204 || reply.status == 304 || reply.status < 200 {
        return false;
    }
    // Never double-encode. A reply that set this itself knows something we do
    // not.
    if reply
        .extra
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("Content-Encoding"))
    {
        return false;
    }

    let Some(compressed) = gzip(&reply.body) else {
        return false;
    };
    // A body that grew is a body sent as-is. Rare for JSON, but a reply that is
    // already compressed data would do exactly this.
    if compressed.len() >= reply.body.len() {
        return false;
    }

    reply.body = compressed;
    reply.extra.push(("Content-Encoding", "gzip".into()));
    // Tells a cache that the response varies by encoding, so a gzip copy is
    // never served to a client that cannot decode it.
    reply.extra.push(("Vary", "Accept-Encoding".into()));
    true
}

fn gzip(data: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(data.len() / 2), LEVEL);
    encoder.write_all(data).ok()?;
    encoder.finish().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(size: usize) -> Vec<u8> {
        // Repetitive, like JSON: compresses well, which is the case that matters.
        b"{\"operational_mode\":\"COOL\",\"target_temperature\":24.0},"
            .iter()
            .cycle()
            .take(size)
            .copied()
            .collect()
    }

    fn reply(size: usize) -> Reply {
        Reply {
            status: 200,
            body: body(size),
            content_type: "application/json",
            extra: Vec::new(),
        }
    }

    #[test]
    fn the_accept_encoding_grammar_is_read_properly() {
        assert!(accepts_gzip(Some("gzip")));
        assert!(accepts_gzip(Some("gzip, deflate")));
        assert!(accepts_gzip(Some("gzip, deflate, br")));
        assert!(accepts_gzip(Some("br, gzip;q=0.8")));
        assert!(accepts_gzip(Some("GZIP")), "case-insensitive");
        assert!(accepts_gzip(Some("*")), "a wildcard accepts anything");
        assert!(accepts_gzip(Some("deflate, *")));

        assert!(!accepts_gzip(None), "silence is not consent");
        assert!(!accepts_gzip(Some("")));
        assert!(!accepts_gzip(Some("deflate")));
        assert!(!accepts_gzip(Some("br")));
        assert!(
            !accepts_gzip(Some("gzip;q=0")),
            "q=0 is an explicit refusal"
        );
        assert!(!accepts_gzip(Some("gzip;q=0.0")));
        assert!(!accepts_gzip(Some("identity")));
    }

    #[test]
    fn a_large_body_is_compressed_and_marked() {
        let mut r = reply(4096);
        let original = r.body.len();
        assert!(maybe_compress(&mut r, Some("gzip, deflate")));
        assert!(r.body.len() < original, "it should be smaller");
        assert!(r
            .extra
            .iter()
            .any(|(n, v)| *n == "Content-Encoding" && v == "gzip"));
        // Vary matters: without it a cache can serve the gzip copy to a client
        // that never asked for one.
        assert!(r
            .extra
            .iter()
            .any(|(n, v)| *n == "Vary" && v == "Accept-Encoding"));
        // And it is real gzip: magic bytes 1f 8b.
        assert_eq!(&r.body[..2], &[0x1f, 0x8b]);
    }

    #[test]
    fn a_small_body_is_left_alone() {
        // Most of this API's traffic. The header and trailer would cost more
        // than the deflate saves.
        let mut r = reply(MIN_SIZE - 1);
        let original = r.body.clone();
        assert!(!maybe_compress(&mut r, Some("gzip")));
        assert_eq!(r.body, original);
        assert!(
            r.extra.is_empty(),
            "no headers added for an uncompressed body"
        );
    }

    #[test]
    fn a_client_that_cannot_decode_gets_it_uncompressed() {
        let mut r = reply(4096);
        let original = r.body.clone();
        assert!(!maybe_compress(&mut r, None));
        assert!(!maybe_compress(&mut r, Some("br")));
        assert_eq!(r.body, original);
    }

    #[test]
    fn an_already_encoded_reply_is_not_touched() {
        // The one that would corrupt a response rather than merely waste time.
        let mut r = reply(4096);
        r.extra.push(("Content-Encoding", "identity".into()));
        let original = r.body.clone();
        assert!(!maybe_compress(&mut r, Some("gzip")));
        assert_eq!(r.body, original);
        assert_eq!(
            r.extra
                .iter()
                .filter(|(n, _)| *n == "Content-Encoding")
                .count(),
            1,
            "no second Content-Encoding"
        );
    }

    #[test]
    fn empty_responses_are_skipped() {
        for status in [204u16, 304] {
            let mut r = Reply {
                status,
                body: Vec::new(),
                content_type: "application/json",
                extra: Vec::new(),
            };
            assert!(!maybe_compress(&mut r, Some("gzip")));
            assert!(r.extra.is_empty());
        }
    }

    #[test]
    fn an_incompressible_body_is_sent_as_is() {
        // Random data deflates to slightly *larger* than the input. Sending that
        // would be a strict loss, so the original wins.
        let mut noise = Vec::with_capacity(4096);
        let mut x: u32 = 0x1234_5678;
        for _ in 0..4096 {
            // xorshift: deterministic, and good enough to defeat deflate.
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            noise.push((x & 0xff) as u8);
        }
        let mut r = Reply {
            status: 200,
            body: noise.clone(),
            content_type: "application/octet-stream",
            extra: Vec::new(),
        };
        assert!(!maybe_compress(&mut r, Some("gzip")));
        assert_eq!(r.body, noise, "the original body survives untouched");
    }

    #[test]
    fn a_compressed_body_round_trips() {
        // The point of the exercise: a client must get its bytes back.
        use std::io::Read;
        let mut r = reply(8192);
        let original = r.body.clone();
        assert!(maybe_compress(&mut r, Some("gzip")));

        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(&r.body[..])
            .read_to_end(&mut decoded)
            .expect("valid gzip");
        assert_eq!(decoded, original);
    }

    #[test]
    fn the_panel_compresses_well() {
        // The case that justifies the dependency: 133 KB of JS and CSS over a
        // phone's WiFi.
        let panel = crate::panel::serve("/js/app.js", None).expect("app.js");
        let original = panel.body.len();
        let mut r = panel;
        assert!(maybe_compress(&mut r, Some("gzip")));
        let ratio = r.body.len() as f64 / original as f64;
        assert!(
            ratio < 0.5,
            "expected better than 2:1 on JavaScript, got {ratio:.2}"
        );
    }
}
