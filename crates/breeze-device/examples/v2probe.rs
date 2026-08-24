//! Do these "V3" units also answer plain V2, with no token at all?
//!
//! Discovery says V3, and V3 wants a `token` and `key` that only Midea's cloud
//! issues — for the account the unit is registered to. If the firmware *also*
//! accepts a bare `0x5A5A` packet, none of that is needed and onboarding becomes
//! a LAN-only affair.
//!
//! Worth trying because the two are not mutually exclusive in the protocol: V3
//! is a session wrapper *around* the same V2 packet, and some firmware answers
//! the inner form directly. It costs one TCP connection to find out.
//!
//! ```text
//! cargo run -p breeze-device --example v2probe 192.168.1.73
//! ```

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use breeze_proto::ac::command;
use breeze_proto::ac::response::State;
use breeze_proto::ac::types::TemperatureType;
use breeze_proto::{frame, packet};

const TIMEOUT: Duration = Duration::from_secs(5);

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(address) = args.next() else {
        eprintln!("usage: v2probe <ip> [device-id]");
        std::process::exit(2);
    };
    // The id goes inside the packet. A wrong one may or may not matter — that is
    // part of what this establishes — so it is optional.
    let device_id: u64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(0);

    let addr: SocketAddr = format!("{address}:6444")
        .parse()
        .expect("address should parse");
    println!("connecting to {addr} (device id {device_id})");

    let mut stream = match TcpStream::connect_timeout(&addr, TIMEOUT) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot connect: {e}");
            std::process::exit(1);
        }
    };
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let _ = stream.set_nodelay(true);

    // A state query, wrapped as a bare V2 packet: no 0x8370 session, no
    // handshake, no token.
    let query = command::get_state(1, TemperatureType::Indoor);
    let outgoing = packet::encode(device_id, &query);
    println!(
        "sending {} bytes, header {:02x?}",
        outgoing.len(),
        &outgoing[..2]
    );

    if let Err(e) = stream.write_all(&outgoing) {
        eprintln!("write failed: {e}");
        std::process::exit(1);
    }

    let mut buffer = vec![0u8; 4096];
    let read = match stream.read(&mut buffer) {
        Ok(0) => {
            println!("\nthe unit closed the connection without answering.");
            println!("=> it will not talk V2. A token and key are required.");
            std::process::exit(1);
        }
        Ok(n) => n,
        Err(e) => {
            println!("\nno answer: {e}");
            println!("=> it ignored a bare V2 packet. A token and key are required.");
            std::process::exit(1);
        }
    };

    let reply = &buffer[..read];
    println!("got {read} bytes, header {:02x?}", &reply[..2.min(read)]);

    match &reply[..2.min(read)] {
        [0x83, 0x70] => {
            println!("\nit answered with a V3 session frame -- so it heard us, but");
            println!("it is speaking the wrapped protocol.");
        }
        [0x5a, 0x5a] => println!("\nit answered with a V2 packet."),
        other => println!("\nunexpected header {other:02x?}"),
    }

    // Try to decode it fully. Getting a real state out is the proof.
    match packet::decode(reply) {
        Ok(inner) => match frame::parse(&inner, frame::DeviceType::AirConditioner) {
            Ok(payload) => match State::parse(payload) {
                Ok(state) => {
                    println!("\nDECODED A REAL STATE with no credentials:");
                    println!("  power={:?} mode={:?}", state.power_on, state.mode);
                    println!(
                        "  target={:?} indoor={:?}",
                        state.target_temperature, state.indoor_temperature
                    );
                    println!("\n=> V2 works. Onboarding needs no cloud at all.");
                    return;
                }
                Err(e) => println!("\npacket and frame decoded, state did not: {e}"),
            },
            Err(e) => println!("\npacket decoded, frame did not: {e}"),
        },
        Err(e) => println!("\ncould not decode as a V2 packet: {e}"),
    }
    println!("=> no usable answer without credentials.");
    std::process::exit(1);
}
