//! Subcommand dispatch.
//!
//! The verb list is not ours to choose. The reference's packaged binary answers
//! `serve`, `pair`, `diag`, `approve`, `devices`, `revoke` and `version`, its
//! postinstall tells people to run `breeze-core pair`, its systemd unit passes
//! `serve --host … --port … $BREEZE_OPTS`, and there are shell aliases in the
//! wild passing `--config /etc/breeze-core/config.json`. A drop-in replacement
//! answers all of that identically; what it adds — `control`, `units`, `login` —
//! is an addition, never a reassignment.
//!
//! `pair` is the one that mattered most: it used to be an alias for `login`
//! here, which meant a new user following the documentation got asked for a
//! server URL and an API key by the command that was supposed to go and find
//! their air conditioners.
//!
//! These also replace the zsh scripts and shell aliases the Python project
//! accumulated. Those were shell because there was nothing else; there is now a
//! binary that already speaks the protocol and holds the credentials, and it
//! works the same on a BSD, on Windows, and on a router with no zsh.
//!
//! `serve` stays the default when no subcommand is given, so an init script that
//! simply execs the binary keeps working.

pub mod admin;
pub mod client;
pub mod control;
pub mod diag;
pub mod pair;
pub mod profile;

use admin::ClientOpts;

/// What the command line asked for.
#[derive(Debug)]
pub enum Command {
    /// Run the server. Flags override the environment.
    Serve {
        host: Option<String>,
        port: Option<u16>,
        behind_proxy: bool,
    },
    Control(Vec<String>),
    Pair {
        ip: Option<String>,
        out: Option<String>,
        prompt: bool,
    },
    Diag {
        client: ClientOpts,
    },
    Login {
        base_url: Option<String>,
    },
    Approve {
        code: Option<String>,
        client: ClientOpts,
    },
    Devices {
        client: ClientOpts,
    },
    Revoke {
        token_id: String,
        client: ClientOpts,
    },
    Units {
        client: ClientOpts,
    },
    Version,
    Help,
}

/// Parse `argv`, minus the program name.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let Some(first) = args.first().map(String::as_str) else {
        // Bare invocation: serve, as an init script would.
        return Ok(Command::Serve {
            host: None,
            port: None,
            behind_proxy: false,
        });
    };
    let rest = &args[1..];

    match first {
        "serve" => {
            let mut host = None;
            let mut port = None;
            let mut behind_proxy = false;
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                match flag.as_str() {
                    // The systemd unit passes these, and ignoring them meant a
                    // deployment that only used flags bound the wrong address.
                    "--host" => host = Some(need(&mut it, "--host")?),
                    "--port" => {
                        let value = need(&mut it, "--port")?;
                        port = Some(
                            value
                                .parse()
                                .map_err(|_| format!("'{value}' is not a port"))?,
                        );
                    }
                    // Load-bearing, not cosmetic: the reference's env file ships
                    // this in BREEZE_OPTS, and a server that ignores it reads
                    // the proxy's own address as the client's — so every request
                    // looks like it came from the LAN and the LAN-only admin
                    // check stops meaning anything at all.
                    "--behind-proxy" => behind_proxy = true,
                    // Accepted and ignored, because the reference's unit file
                    // interpolates a $BREEZE_OPTS that is usually empty.
                    other if other.starts_with('-') => {
                        eprintln!("breeze-core: ignoring unknown option {other}");
                    }
                    other => return Err(format!("unexpected argument '{other}'")),
                }
            }
            Ok(Command::Serve {
                host,
                port,
                behind_proxy,
            })
        }

        "control" => Ok(Command::Control(rest.to_vec())),

        "pair" => {
            let mut ip = None;
            let mut out = None;
            let mut prompt = true;
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                match flag.as_str() {
                    "--ip" => ip = Some(need(&mut it, "--ip")?),
                    "--out" => out = Some(need(&mut it, "--out")?),
                    "--no-prompt" => prompt = false,
                    other => return Err(format!("unexpected argument '{other}'")),
                }
            }
            Ok(Command::Pair { ip, out, prompt })
        }

        "diag" => {
            let mut client = ClientOpts::default();
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                match flag.as_str() {
                    "--base-url" => client.base_url = Some(need(&mut it, "--base-url")?),
                    "--config" => client.config = Some(need(&mut it, "--config")?),
                    // Accepted and ignored, so a script written against the
                    // reference keeps working. This one never prompts anyway.
                    "--auto" => {}
                    other => return Err(format!("unexpected argument '{other}'")),
                }
            }
            Ok(Command::Diag { client })
        }

        // `pair` is deliberately NOT an alias here — it belongs to the units.
        "login" => {
            let mut base_url = None;
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                match flag.as_str() {
                    "--base-url" => base_url = Some(need(&mut it, "--base-url")?),
                    other => return Err(format!("unexpected argument '{other}'")),
                }
            }
            Ok(Command::Login { base_url })
        }

        "approve" => {
            let (code, client) = positional_and_opts(rest)?;
            Ok(Command::Approve { code, client })
        }
        "devices" => {
            let (_, client) = positional_and_opts(rest)?;
            Ok(Command::Devices { client })
        }
        "revoke" => {
            let (token_id, client) = positional_and_opts(rest)?;
            match token_id {
                Some(token_id) => Ok(Command::Revoke { token_id, client }),
                None => Err("revoke needs the token id shown by `breeze-core devices`".into()),
            }
        }
        "units" | "list" => {
            let (_, client) = positional_and_opts(rest)?;
            Ok(Command::Units { client })
        }

        "--version" | "-V" | "version" => Ok(Command::Version),
        "--help" | "-h" | "help" => Ok(Command::Help),
        other => Err(format!("'{other}' is not a command.\n\n{}", usage())),
    }
}

/// One optional positional plus the shared client flags, in any order.
///
/// Order-insensitive because people type it both ways round, and because the
/// alias that has been in use for a year puts the flag first.
fn positional_and_opts(args: &[String]) -> Result<(Option<String>, ClientOpts), String> {
    let mut client = ClientOpts::default();
    let mut positional = None;
    let mut it = args.iter();
    while let Some(argument) = it.next() {
        match argument.as_str() {
            "--base-url" => client.base_url = Some(need(&mut it, "--base-url")?),
            "--config" => client.config = Some(need(&mut it, "--config")?),
            other if other.starts_with("--") => {
                return Err(format!("unexpected argument '{other}'"))
            }
            other if positional.is_none() => positional = Some(other.to_string()),
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }
    Ok((positional, client))
}

fn need<'a, I: Iterator<Item = &'a String>>(it: &mut I, flag: &str) -> Result<String, String> {
    it.next()
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

pub fn usage() -> String {
    format!(
        "breeze-core {} -- LAN-first control for Midea air conditioners

usage:
  breeze-core serve [--host HOST] [--port PORT] [--behind-proxy]
  breeze-core pair [--ip ADDRESS] [--out PATH] [--no-prompt]
  breeze-core control 'NAME' [TYPE] [TEMPERATURE] [FLAP] [FAN] [EXTRA] [TIMER]
  breeze-core diag [--base-url URL] [--config PATH]
  breeze-core units [--config PATH]
  breeze-core approve [CODE] [--config PATH]
  breeze-core devices [--config PATH]
  breeze-core revoke TOKEN_ID [--config PATH]
  breeze-core login [--base-url URL]
  breeze-core --version

  With no arguments at all, it serves -- so an init script may just exec it.

  `pair` finds the air conditioners on the LAN and writes config.json; it is the
  one command that needs no running server. Everything else talks to one over
  its API and enrols itself the first time. `approve`, `devices` and `revoke`
  are the admin side of that and must come from the local network; they read the
  API key from --config when it is given, so they work on the server without a
  profile of their own.

Run `breeze-core control --help` for the control grammar.",
        env!("CARGO_PKG_VERSION")
    )
}

/// List the units, which is the other thing the old aliases were for.
pub fn units(options: &ClientOpts) -> Result<(), String> {
    let client = options.client()?;
    let units = client.get("/api/units")?;
    let list = units.as_array().ok_or("unexpected response")?;
    if list.is_empty() {
        println!("no units configured");
        return Ok(());
    }
    let width = list
        .iter()
        .filter_map(|u| u.get("name")?.as_str().map(str::len))
        .max()
        .unwrap_or(4);
    for unit in list {
        let name = unit.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let ip = unit.get("ip").and_then(|v| v.as_str()).unwrap_or("?");
        let id = unit.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        println!("{name:<width$}  {ip:<15}  {id}");
    }
    Ok(())
}

/// Approve a pending enrolment. The admin half of pairing.
pub fn approve(code: Option<String>, options: &ClientOpts) -> Result<(), String> {
    // Deliberately does *not* require an enrolled device: this is the action
    // that creates the first one, and needing a token to approve a token would
    // be a lock with its key inside. That is what `--config` covers — the API
    // key alone is enough for an admin route.
    let code = match code {
        Some(code) => code,
        None => {
            // The reference prompts when the code is omitted. Do the same, but
            // only when somebody is there to answer: a script that forgot the
            // argument should fail, not hang on a pipe forever.
            use std::io::{BufRead, IsTerminal, Write};
            if !std::io::stdin().is_terminal() {
                return Err("approve needs the code shown by the enrolling client".into());
            }
            print!("pairing code to approve: ");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut line)
                .map_err(|e| format!("cannot read from the terminal: {e}"))?;
            line.trim().to_string()
        }
    };
    if code.is_empty() {
        return Err("no code given".into());
    }

    let client = options.client()?;
    let approved = client.post_json(
        "/api/auth/enroll/approve",
        &serde_json::json!({ "code": code }),
    )?;
    println!(
        "approved {} ({})",
        approved["label"].as_str().unwrap_or("device"),
        approved["token_id"].as_str().unwrap_or("?")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn no_arguments_means_serve() {
        // An init script that simply execs the binary must keep working.
        assert!(matches!(
            parse(&[]).unwrap(),
            Command::Serve {
                host: None,
                port: None,
                behind_proxy: false
            }
        ));
    }

    #[test]
    fn serve_honours_the_flags_the_systemd_unit_passes() {
        match parse(&args("serve --host 127.0.0.1 --port 8420 --behind-proxy")).unwrap() {
            Command::Serve {
                host,
                port,
                behind_proxy,
            } => {
                assert_eq!(host.as_deref(), Some("127.0.0.1"));
                assert_eq!(port, Some(8420));
                // The reference's env file ships this in BREEZE_OPTS. Ignoring
                // it silently defeats the LAN-only admin check.
                assert!(behind_proxy);
            }
            _ => panic!("expected serve"),
        }
    }

    #[test]
    fn serve_tolerates_an_unknown_option_rather_than_refusing_to_start() {
        // A server that will not start because of an option it does not
        // recognise is worse than one that starts and says so.
        assert!(matches!(
            parse(&args("serve --proxy-headers")).unwrap(),
            Command::Serve { .. }
        ));
    }

    #[test]
    fn a_missing_port_value_is_an_error() {
        assert!(parse(&args("serve --port")).is_err());
        assert!(parse(&args("serve --port http")).is_err());
    }

    #[test]
    fn pair_is_device_pairing_and_not_an_alias_for_login() {
        // The whole point: the reference's postinstall says `breeze-core pair`,
        // and it has to go and find air conditioners rather than ask for a
        // server URL.
        match parse(&args(
            "pair --ip 192.168.1.50 --out ./config.json --no-prompt",
        ))
        .unwrap()
        {
            Command::Pair { ip, out, prompt } => {
                assert_eq!(ip.as_deref(), Some("192.168.1.50"));
                assert_eq!(out.as_deref(), Some("./config.json"));
                assert!(!prompt);
            }
            other => panic!("expected pair, got {other:?}"),
        }
        assert!(matches!(
            parse(&args("pair")).unwrap(),
            Command::Pair {
                ip: None,
                out: None,
                prompt: true
            }
        ));
    }

    #[test]
    fn the_admin_commands_take_the_flags_the_shell_aliases_pass() {
        // acapprove='breeze-core approve --config /etc/breeze-core/config.json'
        match parse(&args(
            "approve --config /etc/breeze-core/config.json ABCD-EFGH",
        ))
        .unwrap()
        {
            Command::Approve { code, client } => {
                assert_eq!(code.as_deref(), Some("ABCD-EFGH"));
                assert_eq!(
                    client.config.as_deref(),
                    Some("/etc/breeze-core/config.json")
                );
            }
            _ => panic!("expected approve"),
        }
        // ...and in the other order, because a person types it either way.
        match parse(&args("approve ABCD-EFGH --base-url http://127.0.0.1:8420")).unwrap() {
            Command::Approve { code, client } => {
                assert_eq!(code.as_deref(), Some("ABCD-EFGH"));
                assert_eq!(client.base_url.as_deref(), Some("http://127.0.0.1:8420"));
            }
            _ => panic!("expected approve"),
        }
    }

    #[test]
    fn diag_accepts_the_references_flags() {
        // acdiag='breeze-core diag --base-url … --config …'
        match parse(&args(
            "diag --base-url http://127.0.0.1:8420 --config /etc/breeze-core/config.json --auto",
        ))
        .unwrap()
        {
            Command::Diag { client } => {
                assert_eq!(client.base_url.as_deref(), Some("http://127.0.0.1:8420"));
                assert!(client.config.is_some());
            }
            _ => panic!("expected diag"),
        }
    }

    #[test]
    fn devices_and_revoke_exist_because_the_reference_has_them() {
        assert!(matches!(
            parse(&args("devices --config /etc/breeze-core/config.json")).unwrap(),
            Command::Devices { .. }
        ));
        match parse(&args("revoke a1b2c3")).unwrap() {
            Command::Revoke { token_id, .. } => assert_eq!(token_id, "a1b2c3"),
            _ => panic!("expected revoke"),
        }
        // A revoke with no id would otherwise revoke nothing and say so as if
        // it had worked.
        assert!(parse(&args("revoke")).is_err());
    }

    #[test]
    fn login_is_not_reachable_through_pair() {
        assert!(matches!(
            parse(&args("login")).unwrap(),
            Command::Login { .. }
        ));
        assert!(!matches!(
            parse(&args("pair")).unwrap(),
            Command::Login { .. }
        ));
    }

    #[test]
    fn control_takes_everything_after_it_verbatim() {
        match parse(&args("control kitchen cool 25.5")).unwrap() {
            Command::Control(rest) => assert_eq!(rest, args("kitchen cool 25.5")),
            _ => panic!("expected control"),
        }
    }

    #[test]
    fn an_unknown_command_shows_the_usage() {
        let error = parse(&args("frobnicate")).unwrap_err();
        assert!(error.contains("is not a command"), "{error}");
        assert!(error.contains("breeze-core control"), "{error}");
    }

    #[test]
    fn the_usage_lists_every_verb_the_reference_answers() {
        // A verb missing from the usage is a verb somebody's script is about to
        // find out is missing.
        let text = usage();
        for verb in [
            "serve", "pair", "diag", "units", "approve", "devices", "revoke", "login",
        ] {
            assert!(
                text.contains(&format!("breeze-core {verb}")),
                "missing {verb}"
            );
        }
    }
}
