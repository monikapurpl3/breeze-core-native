//! Finding units on the LAN — the socket half of discovery.
//!
//! # Why this does not just broadcast
//!
//! The obvious implementation sends one hello to `255.255.255.255:6445` and
//! collects the answers. That is what the reference does, and on this
//! maintainer's network it finds **nothing** — while direct control of the same
//! three units works perfectly.
//!
//! Measured rather than guessed at. The units answer a *unicast* hello every
//! time, on 6445 (never on 20086). Their answers to a *broadcast* hello never
//! arrive. The reason is that a reply to a broadcast is an ordinary unicast
//! packet from the unit's own address, while the conntrack entry the query
//! created was for the broadcast address — so the reply matches nothing and a
//! default-deny INPUT policy discards it. Priming one entry with a unicast
//! first made exactly one unit appear in a subsequent broadcast, which is the
//! signature of that and of nothing else.
//!
//! So this sends **both**: a broadcast, which is cheap and works where the
//! network allows it, and a unicast hello to every address in the local subnet.
//! Each unicast reply matches a conntrack entry it created itself, so it arrives
//! without a firewall rule — and the sweep also survives an access point that
//! filters broadcast toward wireless clients, which is the other common cause of
//! the same symptom.
//!
//! A /24 is 254 packets of 72 bytes: about 18 KB, once, when a human presses
//! "scan". That is a rounding error on a LAN and it is the difference between
//! automatic pairing working and not.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::time::{Duration, Instant};

use breeze_proto::discover::{self, Discovered, DISCOVERY_MSG, DISCOVERY_PORTS};

/// How long to keep listening after the last hello goes out.
///
/// Generous because a unit answers when it feels like it: the units measured
/// here replied within ~200 ms, but a busy or sleeping one can take longer, and
/// a scan that misses a unit sends someone hunting for its IP by hand.
pub const DEFAULT_LISTEN: Duration = Duration::from_secs(4);

/// Never sweep anything bigger than this. A /24 is 254 hosts; the cap allows up
/// to a /22 and refuses the `/8` somebody will eventually type by mistake.
pub const MAX_SWEEP_HOSTS: u32 = 1024;

#[derive(Debug)]
pub enum ScanError {
    /// No usable socket, or the address could not be worked out.
    Io(std::io::Error),
    /// The subnet was public, or too large to sweep.
    BadSubnet(String),
}

impl core::fmt::Display for ScanError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::BadSubnet(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for ScanError {}

impl From<std::io::Error> for ScanError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// What a scan turned up, plus enough about the attempt to explain a blank
/// result to a person.
#[derive(Debug, Default)]
pub struct ScanReport {
    /// One entry per unit, keyed by id so a unit answering on both ports is
    /// reported once.
    pub found: Vec<Discovered>,
    /// Replies that arrived but could not be parsed, with the reason. Surfaced
    /// rather than swallowed: "three things answered and I understood none of
    /// them" is a very different problem from "nothing answered".
    pub unparseable: Vec<(Ipv4Addr, String)>,
    /// How many unicast probes went out.
    pub probes_sent: u32,
    /// Whether the broadcast could even be sent.
    pub broadcast_sent: bool,
    /// The subnet that was swept, if one was worked out.
    pub swept: Option<String>,
}

/// The server's own private IPv4 address.
///
/// Uses the connect-a-datagram trick: connecting a UDP socket sets the local
/// address from the routing table without sending anything, so it works with no
/// egress at all and regardless of what the HTTP listener is bound to.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    for probe in ["192.168.1.1:9", "10.0.0.1:9", "172.16.0.1:9"] {
        let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
            continue;
        };
        if socket.connect(probe).is_err() {
            continue;
        }
        if let Ok(SocketAddr::V4(local)) = socket.local_addr() {
            let ip = *local.ip();
            if ip.is_private() && !ip.is_loopback() {
                return Some(ip);
            }
        }
    }
    None
}

/// Every host address in the `/24` around `ip`.
fn hosts_of_24(ip: Ipv4Addr) -> Vec<Ipv4Addr> {
    let [a, b, c, _] = ip.octets();
    (1..=254).map(|d| Ipv4Addr::new(a, b, c, d)).collect()
}

/// Discover units on the local subnet.
///
/// Sends a broadcast *and* a unicast sweep, then listens once for everything.
/// Both, because which one works depends on the host: from the maintainer's
/// server the broadcast replies are dropped and only the sweep finds anything,
/// while from their desktop on the same LAN the broadcast answers fine.
pub fn scan(listen: Duration) -> Result<ScanReport, ScanError> {
    let local = local_ipv4();
    let hosts = match local {
        Some(ip) => hosts_of_24(ip),
        // No idea what the subnet is: the broadcast is still worth sending.
        None => Vec::new(),
    };
    scan_targets(&hosts, local.map(|ip| format!("{ip}/24")), true, listen)
}

/// Discover units, sweeping an explicit subnet in CIDR form.
///
/// Deliberately does **not** broadcast: a broadcast goes to the local link,
/// which has nothing to do with the subnet that was asked for. Somebody naming
/// a subnet wants that subnet probed, and a result from somewhere else would be
/// a confusing answer to a precise question.
pub fn scan_subnet(cidr: &str, listen: Duration) -> Result<ScanReport, ScanError> {
    let (hosts, label) = parse_cidr(cidr)?;
    scan_targets(&hosts, Some(label), false, listen)
}

/// Parse `a.b.c.d/len` into its host addresses.
///
/// Refuses anything public, and anything larger than [`MAX_SWEEP_HOSTS`]:
/// sweeping the internet is never what was meant, and a `/8` typo would send a
/// million packets.
fn parse_cidr(cidr: &str) -> Result<(Vec<Ipv4Addr>, String), ScanError> {
    let (addr, prefix) = cidr
        .split_once('/')
        .ok_or_else(|| ScanError::BadSubnet(format!("'{cidr}' is not a CIDR subnet")))?;
    let addr: Ipv4Addr = addr
        .trim()
        .parse()
        .map_err(|_| ScanError::BadSubnet(format!("'{addr}' is not an IPv4 address")))?;
    let prefix: u32 = prefix
        .trim()
        .parse()
        .map_err(|_| ScanError::BadSubnet(format!("'{prefix}' is not a prefix length")))?;
    if prefix > 32 {
        return Err(ScanError::BadSubnet(format!("/{prefix} is not a prefix")));
    }
    if !addr.is_private() {
        return Err(ScanError::BadSubnet(
            "refusing to scan a non-private subnet".into(),
        ));
    }

    let host_bits = 32 - prefix;
    let count = 1u64 << host_bits;
    if count > MAX_SWEEP_HOSTS as u64 {
        return Err(ScanError::BadSubnet(format!(
            "subnet too large: {count} addresses, the limit is {MAX_SWEEP_HOSTS}"
        )));
    }

    let base = u32::from(addr) & (!0u32).checked_shl(host_bits).unwrap_or(0);
    let mut hosts = Vec::new();
    for offset in 0..count as u32 {
        let candidate = Ipv4Addr::from(base + offset);
        // Skip the network and broadcast addresses of anything bigger than a /31.
        if host_bits >= 2 && (offset == 0 || offset == count as u32 - 1) {
            continue;
        }
        hosts.push(candidate);
    }
    Ok((hosts, format!("{addr}/{prefix}")))
}

/// The shared body: probe, then listen once.
fn scan_targets(
    hosts: &[Ipv4Addr],
    swept: Option<String>,
    broadcast: bool,
    listen: Duration,
) -> Result<ScanReport, ScanError> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_broadcast(true)?;
    // Short, not zero: a blocking read would hang past the deadline, and a busy
    // loop would spin a core for the whole listen window.
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;

    let mut report = ScanReport {
        swept,
        ..Default::default()
    };

    // Send and listen at the same time, on two threads sharing the socket.
    //
    // Sequentially, a /24 measured 6.3s of sending before listening even began,
    // for a 10.3s scan. Worse than slow: replies to the first probes sit in the
    // receive buffer for the whole send phase, and on a network with many units
    // the earliest answers are the ones at risk of being dropped. Overlapping
    // them means the scan costs about as long as the sending alone.
    let probes = AtomicU32::new(0);
    let broadcast_ok = AtomicBool::new(false);
    let sending_done = AtomicBool::new(false);
    let order = std::sync::atomic::Ordering::Relaxed;

    // Keyed by id so a unit that answers on both ports -- or answers the
    // broadcast and its own unicast probe -- lands once.
    let mut seen: BTreeMap<u64, Discovered> = BTreeMap::new();
    let mut unparseable: Vec<(Ipv4Addr, String)> = Vec::new();

    std::thread::scope(|scope| {
        scope.spawn(|| {
            for port in DISCOVERY_PORTS {
                // Broadcast first, so a network that allows it answers while
                // the sweep is still going out.
                if broadcast
                    && socket
                        .send_to(
                            &DISCOVERY_MSG,
                            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, port)),
                        )
                        .is_ok()
                {
                    broadcast_ok.store(true, order);
                }
                for host in hosts {
                    // A send failure here is normal and uninteresting: an
                    // address with nothing behind it can fail to resolve.
                    if socket
                        .send_to(
                            &DISCOVERY_MSG,
                            SocketAddr::V4(SocketAddrV4::new(*host, port)),
                        )
                        .is_ok()
                    {
                        probes.fetch_add(1, order);
                    }
                }
            }
            sending_done.store(true, order);
        });

        let mut buffer = [0u8; 2048];
        // Set once sending finishes: keep listening for `listen` past the last
        // probe, rather than from the start, or a slow sweep would eat the
        // whole window before the last unit was even asked.
        let mut deadline: Option<Instant> = None;
        loop {
            if deadline.is_none() && sending_done.load(order) {
                deadline = Some(Instant::now() + listen);
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
            let (len, from) = match socket.recv_from(&mut buffer) {
                Ok(v) => v,
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue
                }
                // A host that refused an earlier probe can surface an ICMP
                // error here. Not fatal to the scan.
                Err(_) => continue,
            };
            let IpAddr::V4(src) = from.ip() else { continue };

            match discover::parse(src, &buffer[..len]) {
                Ok(found) => {
                    seen.entry(found.id).or_insert(found);
                }
                Err(e) => {
                    // Once per address, so one chatty device cannot bury the
                    // report.
                    if !unparseable.iter().any(|(ip, _)| *ip == src) {
                        unparseable.push((src, e.to_string()));
                    }
                }
            }
        }
    });

    report.probes_sent = probes.load(order);
    report.broadcast_sent = broadcast_ok.load(order);
    report.unparseable = unparseable;
    report.found = seen.into_values().collect();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_24_sweep_covers_every_host_and_neither_edge() {
        let hosts = hosts_of_24(Ipv4Addr::new(192, 168, 1, 98));
        assert_eq!(hosts.len(), 254);
        assert_eq!(hosts[0], Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(hosts[253], Ipv4Addr::new(192, 168, 1, 254));
        assert!(
            !hosts.contains(&Ipv4Addr::new(192, 168, 1, 0)),
            "the network address is not a host"
        );
        assert!(
            !hosts.contains(&Ipv4Addr::new(192, 168, 1, 255)),
            "the broadcast address is sent to separately, not swept"
        );
        // Includes the server's own address, deliberately: harmless, and a unit
        // could in principle share it via a router doing odd things.
        assert!(hosts.contains(&Ipv4Addr::new(192, 168, 1, 98)));
    }

    #[test]
    fn a_cidr_sweep_matches_the_24_helper() {
        let (hosts, label) = parse_cidr("192.168.1.0/24").unwrap();
        assert_eq!(label, "192.168.1.0/24");
        assert_eq!(hosts, hosts_of_24(Ipv4Addr::new(192, 168, 1, 1)));
    }

    #[test]
    fn a_public_subnet_is_refused() {
        // Never scan address space that is not the operator's own.
        let err = parse_cidr("8.8.8.0/24").unwrap_err();
        assert!(err.to_string().contains("non-private"), "got {err}");
    }

    #[test]
    fn an_oversized_subnet_is_refused_before_any_packet() {
        // The /8 somebody will type by accident: 16 million probes.
        let err = parse_cidr("10.0.0.0/8").unwrap_err();
        assert!(err.to_string().contains("too large"), "got {err}");
        // A /22 is the documented ceiling and must still be allowed.
        assert!(parse_cidr("10.1.0.0/22").is_ok());
        assert!(parse_cidr("10.1.0.0/21").is_err());
    }

    #[test]
    fn malformed_subnets_are_refused_with_a_reason() {
        for bad in [
            "192.168.1.0",
            "not-an-address/24",
            "192.168.1.0/x",
            "192.168.1.0/33",
            "",
        ] {
            let err = parse_cidr(bad).unwrap_err();
            assert!(
                !err.to_string().is_empty(),
                "{bad} should be refused with an explanation"
            );
        }
    }

    #[test]
    fn a_small_subnet_keeps_both_of_its_two_addresses() {
        // A /31 is a point-to-point link: both addresses are usable, so the
        // network/broadcast skip must not apply.
        let (hosts, _) = parse_cidr("192.168.1.0/31").unwrap();
        assert_eq!(hosts.len(), 2);
        let (hosts, _) = parse_cidr("192.168.1.5/32").unwrap();
        assert_eq!(hosts, vec![Ipv4Addr::new(192, 168, 1, 5)]);
    }

    #[test]
    fn a_30_subnet_drops_its_edges() {
        let (hosts, _) = parse_cidr("192.168.1.0/30").unwrap();
        assert_eq!(
            hosts,
            vec![Ipv4Addr::new(192, 168, 1, 1), Ipv4Addr::new(192, 168, 1, 2)]
        );
    }

    #[test]
    fn the_cidr_base_is_masked_rather_than_trusted() {
        // "192.168.1.130/24" means the whole /24, not a range starting at 130.
        let (from_host, _) = parse_cidr("192.168.1.130/24").unwrap();
        let (from_base, _) = parse_cidr("192.168.1.0/24").unwrap();
        assert_eq!(from_host, from_base);
    }

    #[test]
    fn scanning_a_dead_subnet_returns_empty_rather_than_failing() {
        // Exercises the real socket path without hardware. It must not
        // broadcast: the first version of this test did, and on the developer's
        // own LAN it found an actual air conditioner and failed -- which is how
        // the "an explicit subnet means only that subnet" rule got written.
        let report = scan_subnet("192.168.199.0/30", Duration::from_millis(300))
            .expect("a scan with no answers is not an error");
        assert!(
            !report.broadcast_sent,
            "an explicit subnet must not reach outside itself"
        );
        assert!(report.found.is_empty(), "found {:?}", report.found);
        assert_eq!(report.swept.as_deref(), Some("192.168.199.0/30"));
        // Two hosts, times the two discovery ports.
        assert!(report.probes_sent <= 4);
    }
}
