//! Discovery parsing, checked against a reply captured from real hardware.
//!
//! `fixtures/discovery-reply-v3.bin` is 208 bytes straight off the wire from a
//! Korel unit (Midea underneath) at 192.168.1.72, answering the standard hello
//! on UDP 6445. It is here because it is the only discovery input in the project
//! that nobody wrote: msmart's vectors cover the packet layers, and a fixture I
//! made up would only prove I can parse my own fixture.
//!
//! It also documents what these units actually do, which is not what the
//! reference implementation assumes:
//!
//! * they answer on **6445 only** — 20086 got nothing from any of the three;
//! * they answer a **unicast** hello reliably, and their answers to a broadcast
//!   never arrive on this network, which is why automatic pairing found nothing.

use breeze_proto::discover::{self, Version};
use std::net::Ipv4Addr;

const REPLY: &[u8] = include_bytes!("fixtures/discovery-reply-v3.bin");

/// The unit's id as recorded in the deployment's own `config.json`, which was
/// written by the vendor-blessed pairing path. If parsing agrees with that, the
/// 48-bit little-endian field is being read correctly.
const KNOWN_ID: u64 = 152832116843678;

#[test]
fn a_real_v3_reply_is_recognised_as_v3() {
    assert_eq!(REPLY.len(), 208, "the captured reply is 208 bytes");
    assert_eq!(&REPLY[..2], &[0x83, 0x70], "V3 wraps its reply in 0x8370");
    assert_eq!(discover::version_of(REPLY).unwrap(), Version::V3);
}

#[test]
fn a_real_v3_reply_parses_to_the_unit_we_know_is_there() {
    let src = Ipv4Addr::new(192, 168, 1, 72);
    let found = discover::parse(src, REPLY).expect("a real reply must parse");

    assert_eq!(found.version, Version::V3);
    assert_eq!(
        found.id, KNOWN_ID,
        "id must match what the deployment's config.json records for this unit"
    );
    assert_eq!(
        found.ip, src,
        "the address we received from is authoritative"
    );
    assert_eq!(
        found.reported_ip, src,
        "and this unit agrees about its own address"
    );
    assert_eq!(found.port, 6444, "control is on 6444, discovery on 6445");

    // An SSID of the form `net_ac_XXXX`, whose middle field is the appliance
    // type. 0xAC is an air conditioner.
    assert!(
        found.ssid.starts_with("net_ac_"),
        "unexpected SSID {:?}",
        found.ssid
    );
    assert_eq!(found.device_type, 0xac);
    assert_eq!(
        found.serial_number.len(),
        32,
        "serial is a fixed 32-character field, got {:?}",
        found.serial_number
    );
}

#[test]
fn truncating_a_real_reply_is_an_error_rather_than_a_panic() {
    // Every length short of the full reply, to be sure no slice is unchecked.
    // A malformed UDP packet from anywhere on the LAN reaches this parser.
    for cut in 0..REPLY.len() {
        let _ = discover::parse(Ipv4Addr::LOCALHOST, &REPLY[..cut]);
    }
}

#[test]
fn the_id_is_plaintext_and_the_rest_is_not() {
    // Worth pinning because it is counter-intuitive and it shapes what the
    // encryption is protecting: the unit id sits in the clear at bytes 20..26 of
    // the inner packet, while the address, port, serial and SSID are inside the
    // encrypted body. So a corrupted body still yields the right id -- there is
    // nothing to detect it with -- and that is the reply's design, not a bug.
    let src = Ipv4Addr::new(192, 168, 1, 72);
    let good = discover::parse(src, REPLY).unwrap();

    let mut broken = REPLY.to_vec();
    // Past the 8-byte V3 header and the 40-byte plaintext preamble, so this
    // lands inside the encrypted region.
    broken[8 + 45] ^= 0xff;

    match discover::parse(src, &broken) {
        // Refused outright: also fine, and the more likely outcome when the
        // damaged block is the one holding the SSID.
        Err(_) => {}
        Ok(corrupted) => {
            assert_eq!(
                corrupted.id, good.id,
                "the id is plaintext, so it survives body corruption"
            );
            assert_ne!(
                (
                    corrupted.serial_number,
                    corrupted.ssid,
                    corrupted.reported_ip
                ),
                (good.serial_number, good.ssid, good.reported_ip),
                "something inside the encrypted body must have changed"
            );
        }
    }
}
