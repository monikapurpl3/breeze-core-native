//! Subcommand dispatch.
//!
//! Replaces the zsh scripts and shell aliases the Python project accumulated:
//! `acdiag`, and the assorted one-liners for poking a unit. Those were shell
//! because there was nothing else; there is now a binary that already speaks the
//! protocol and holds the credentials, so it may as well do the job — and it
//! works the same on a BSD, on Windows, and on a router with no zsh.
//!
//! `serve` stays the default when no subcommand is given, so an init script that
//! simply execs the binary keeps working.

pub mod client;
pub mod control;
pub mod diag;
pub mod profile;

/// What the command line asked for.
#[derive(Debug)]
pub enum Command {
    /// Run the server. Flags override the environment.
    Serve {
        host: Option<String>,
        port: Option<u16>,
    },
    Control(Vec<String>),
    Diag {
        base_url: Option<String>,
    },
    Login {
        base_url: Option<String>,
    },
    Approve(String),
    Units,
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
        });
    };

    match first {
        "serve" => {
            let mut host = None;
            let mut port = None;
            let mut rest = args[1..].iter();
            while let Some(flag) = rest.next() {
                match flag.as_str() {
                    // The systemd unit passes these, and ignoring them meant a
                    // deployment that only used flags bound the wrong address.
                    "--host" => host = rest.next().cloned(),
                    "--port" => {
                        let value = rest.next().ok_or("--port needs a number")?;
                        port = Some(
                            value
                                .parse()
                                .map_err(|_| format!("'{value}' is not a port"))?,
                        );
                    }
                    // Accepted and ignored, because the reference's unit file
                    // interpolates a $BREEZE_OPTS that is usually empty.
                    other if other.starts_with('-') => {
                        eprintln!("breeze-core: ignoring unknown option {other}");
                    }
                    other => return Err(format!("unexpected argument '{other}'")),
                }
            }
            Ok(Command::Serve { host, port })
        }
        "control" => Ok(Command::Control(args[1..].to_vec())),
        "diag" => {
            let mut base_url = None;
            let mut rest = args[1..].iter();
            while let Some(flag) = rest.next() {
                match flag.as_str() {
                    "--base-url" => base_url = rest.next().cloned(),
                    // Accepted and ignored, so a script written against the
                    // Python CLI keeps working. This one never prompts anyway.
                    "--auto" => {}
                    other => return Err(format!("unexpected argument '{other}'")),
                }
            }
            Ok(Command::Diag { base_url })
        }
        "login" | "pair" => {
            let mut base_url = None;
            let mut rest = args[1..].iter();
            while let Some(flag) = rest.next() {
                match flag.as_str() {
                    "--base-url" => base_url = rest.next().cloned(),
                    other => return Err(format!("unexpected argument '{other}'")),
                }
            }
            Ok(Command::Login { base_url })
        }
        "approve" => args
            .get(1)
            .map(|code| Command::Approve(code.clone()))
            .ok_or_else(|| "approve needs the code shown by the enrolling client".to_string()),
        "units" | "list" => Ok(Command::Units),
        "--version" | "-V" | "version" => Ok(Command::Version),
        "--help" | "-h" | "help" => Ok(Command::Help),
        other => Err(format!("'{other}' is not a command.\n\n{}", usage())),
    }
}

pub fn usage() -> String {
    format!(
        "breeze-core {} -- LAN-first control for Midea air conditioners

usage:
  breeze-core serve [--host HOST] [--port PORT]
  breeze-core control 'NAME' [TYPE] [TEMPERATURE] [FLAP] [FAN] [EXTRA] [TIMER]
  breeze-core diag [--base-url URL]
  breeze-core units
  breeze-core login [--base-url URL]
  breeze-core approve CODE
  breeze-core --version

  With no arguments at all, it serves -- so an init script may just exec it.

  `control` and `diag` talk to a running server over its API, and enrol
  themselves the first time. `approve` is the admin half of that, and has to be
  run from the local network.

Run `breeze-core control --help` for the control grammar.",
        env!("CARGO_PKG_VERSION")
    )
}

/// List the units, which is the other thing the old aliases were for.
pub fn units() -> Result<(), String> {
    let profile = profile::ensure()?;
    let client = client::Client::from_profile(&profile);
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
pub fn approve(code: &str) -> Result<(), String> {
    // Deliberately does *not* need an enrolled device: this is the action that
    // creates the first one, and requiring a token to approve a token would be
    // a lock with the key inside.
    let profile = profile::load();
    let base_url = profile
        .as_ref()
        .map(|p| p.base_url.clone())
        .unwrap_or_else(|| profile::DEFAULT_BASE_URL.to_string());
    let api_key = match profile.as_ref().map(|p| p.api_key.clone()) {
        Some(key) => key,
        None => return Err("no profile yet; run `breeze-core login` first".into()),
    };

    let client = client::Client::new(base_url, api_key, None);
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
                port: None
            }
        ));
    }

    #[test]
    fn serve_honours_the_flags_the_systemd_unit_passes() {
        // These were being ignored, so a deployment that set only flags -- no
        // environment -- bound the default address instead of the one asked for.
        match parse(&args("serve --host 127.0.0.1 --port 8420")).unwrap() {
            Command::Serve { host, port } => {
                assert_eq!(host.as_deref(), Some("127.0.0.1"));
                assert_eq!(port, Some(8420));
            }
            _ => panic!("expected serve"),
        }
    }

    #[test]
    fn serve_tolerates_an_unknown_option_rather_than_refusing_to_start() {
        // The reference's unit file interpolates $BREEZE_OPTS, and a server that
        // will not start because of an option it does not recognise is worse
        // than one that starts and says so.
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
    fn control_takes_everything_after_it_verbatim() {
        match parse(&args("control kuhinja cool 25.5")).unwrap() {
            Command::Control(rest) => assert_eq!(rest, args("kuhinja cool 25.5")),
            _ => panic!("expected control"),
        }
    }

    #[test]
    fn diag_accepts_the_pythons_flags() {
        match parse(&args("diag --base-url http://x:8421 --auto")).unwrap() {
            Command::Diag { base_url } => {
                assert_eq!(base_url.as_deref(), Some("http://x:8421"));
            }
            _ => panic!("expected diag"),
        }
    }

    #[test]
    fn login_and_pair_are_the_same_thing() {
        assert!(matches!(
            parse(&args("login")).unwrap(),
            Command::Login { .. }
        ));
        assert!(matches!(
            parse(&args("pair")).unwrap(),
            Command::Login { .. }
        ));
    }

    #[test]
    fn approve_needs_its_code() {
        assert!(parse(&args("approve")).is_err());
        match parse(&args("approve ABCD-EFGH")).unwrap() {
            Command::Approve(code) => assert_eq!(code, "ABCD-EFGH"),
            _ => panic!("expected approve"),
        }
    }

    #[test]
    fn an_unknown_command_shows_the_usage() {
        let error = parse(&args("frobnicate")).unwrap_err();
        assert!(error.contains("is not a command"), "{error}");
        assert!(error.contains("breeze-core control"), "{error}");
    }
}
