//! `breeze-core proxy` -- put Breeze Core behind nginx, Apache or Caddy, with
//! HTTPS, one explained step at a time.
//!
//! It does the work, and asks first every time: it detects what is installed,
//! explains the DNS record and checks that it points here, shows each file
//! before writing it, backs up anything it replaces, validates the
//! configuration before reloading, gets the certificate (certbot, or Caddy
//! itself), and moves Breeze Core behind the proxy. Every change goes into a
//! journal, `<config dir>/proxy-undo.json`, which `breeze-core proxy --undo`
//! plays backwards. `--dry-run` shows all of it and changes nothing.
//!
//! The configurations are the ones in the wiki's "Reverse proxy and TLS",
//! which hold to the four rules a proxy in front of this must keep:
//! X-Forwarded-For overwritten (never appended), `--behind-proxy` on the
//! server, no buffering on the event stream, security headers in one place.
//!
//! Linux only. Windows has its own wizard, which the installer puts next to
//! the binary; the BSDs, OPNsense and Termux are pointed at the wiki.

/// What the command line asked for.
#[derive(Debug)]
pub struct Options {
    pub dry_run: bool,
    pub undo: bool,
    /// `nginx`, `apache` or `caddy`, to skip the question.
    pub server: Option<String>,
    pub domain: Option<String>,
}

pub fn run(options: Options) -> Result<i32, String> {
    #[cfg(target_os = "linux")]
    return linux::run(options);

    #[cfg(windows)]
    return windows(options);

    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = options;
        Err(format!(
            "`breeze-core proxy` sets up nginx, Apache or Caddy on Linux. On {} the \
             configuration is in the wiki, ready to paste: \
             https://github.com/monikapurpl3/breeze-core-native/wiki/Reverse-proxy-and-TLS",
            std::env::consts::OS
        ))
    }
}

/// Windows: the installer's own Caddy wizard, which lives next to the binary.
#[cfg(windows)]
fn windows(options: Options) -> Result<i32, String> {
    let wizard = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("caddy-wizard.ps1")))
        .filter(|p| p.is_file())
        .ok_or(
            "on Windows this is the installer's Caddy wizard (Start menu: Breeze Core > Set up \
             Caddy reverse proxy), and caddy-wizard.ps1 is not next to this breeze-core.exe",
        )?;
    let mut command = std::process::Command::new("powershell.exe");
    command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
    command.arg(&wizard);
    if let Some(domain) = &options.domain {
        command.args(["-Domain", domain]);
    }
    if options.dry_run {
        command.arg("-DryRun");
    }
    let status = command
        .status()
        .map_err(|e| format!("cannot start PowerShell: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io::{BufRead, Read, Write};
    use std::net::{IpAddr, TcpStream, ToSocketAddrs};
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;

    use serde::{Deserialize, Serialize};

    use super::Options;

    // --- the parts ---------------------------------------------------------------

    #[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum Kind {
        Nginx,
        Apache,
        Caddy,
    }

    impl Kind {
        fn name(self) -> &'static str {
            match self {
                Self::Nginx => "nginx",
                Self::Apache => "Apache",
                Self::Caddy => "Caddy",
            }
        }
    }

    /// The package manager, for installing a server or certbot.
    #[derive(Clone, Copy, PartialEq)]
    enum Pm {
        Apt,
        Dnf,
        Pacman,
        Apk,
        Zypper,
    }

    impl Pm {
        fn detect() -> Option<Self> {
            [
                ("apt-get", Self::Apt),
                ("dnf", Self::Dnf),
                ("pacman", Self::Pacman),
                ("apk", Self::Apk),
                ("zypper", Self::Zypper),
            ]
            .into_iter()
            .find(|(bin, _)| on_path(bin))
            .map(|(_, pm)| pm)
        }

        fn install(self, packages: &[&str]) -> Vec<String> {
            let mut c: Vec<String> = match self {
                Self::Apt => vec!["apt-get", "install", "-y"],
                Self::Dnf => vec!["dnf", "install", "-y"],
                Self::Pacman => vec!["pacman", "-S", "--needed", "--noconfirm"],
                Self::Apk => vec!["apk", "add"],
                Self::Zypper => vec!["zypper", "--non-interactive", "install"],
            }
            .into_iter()
            .map(String::from)
            .collect();
            c.extend(packages.iter().map(|p| p.to_string()));
            c
        }

        /// The package names for a server here, where this wizard supports it.
        fn server_packages(self, kind: Kind) -> Option<Vec<&'static str>> {
            Some(match (kind, self) {
                (Kind::Nginx, _) => vec!["nginx"],
                (Kind::Caddy, _) => vec!["caddy"],
                (Kind::Apache, Self::Apt) => vec!["apache2"],
                (Kind::Apache, Self::Dnf) => vec!["httpd", "mod_ssl"],
                (Kind::Apache, _) => return None,
            })
        }

        fn certbot_packages(self, kind: Kind) -> Vec<&'static str> {
            let plugin = match (kind, self) {
                (Kind::Apache, Self::Pacman | Self::Apk) => "certbot-apache",
                (Kind::Apache, _) => "python3-certbot-apache",
                (_, Self::Pacman | Self::Apk) => "certbot-nginx",
                _ => "python3-certbot-nginx",
            };
            vec!["certbot", plugin]
        }
    }

    /// One change made, kept so it can be played backwards.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum Step {
        Created {
            path: String,
        },
        Replaced {
            path: String,
            backup: String,
        },
        /// A command whose effect another command reverses (a2ensite).
        Ran {
            command: Vec<String>,
            undo: Vec<String>,
        },
    }

    #[derive(Serialize, Deserialize, Debug, Default)]
    struct Journal {
        server: Option<Kind>,
        domain: String,
        steps: Vec<Step>,
    }

    struct Ctx {
        dry_run: bool,
        journal: Journal,
        pm: Option<Pm>,
    }

    fn config_dir() -> String {
        breeze_store::config_dir()
    }

    fn journal_path() -> String {
        format!("{}/proxy-undo.json", config_dir())
    }

    fn env_path() -> String {
        format!("{}/breeze-core.env", config_dir())
    }

    // --- the wizard ----------------------------------------------------------------

    pub fn run(options: Options) -> Result<i32, String> {
        if options.undo {
            return undo(options.dry_run);
        }
        if !options.dry_run && !is_root() {
            return Err(
                "this changes the web server's configuration and Breeze Core's own, so it \
                 needs root: run it with sudo. To see everything it would do first, as \
                 anyone: breeze-core proxy --dry-run"
                    .into(),
            );
        }
        if Path::new(&journal_path()).exists() {
            say(&format!(
                "A previous run left a journal at {}. Run `breeze-core proxy --undo` first to \
                 take that set-up back out, then this again.",
                journal_path()
            ));
            return Ok(1);
        }
        let mut ctx = Ctx {
            dry_run: options.dry_run,
            journal: Journal::default(),
            pm: Pm::detect(),
        };

        heading("Putting Breeze Core behind a reverse proxy");
        say(
            "This puts a web server in front of Breeze Core, so it can be reached by name, over \
             HTTPS, from outside your home. Breeze Core itself only speaks plain HTTP, on \
             purpose: certificates are a web server's job.",
        );
        say(
            "Before going further: a VPN into your home network is the safer way to reach it \
             from outside, and needs none of this. If you would rather do this, here is what \
             happens, and nothing is changed without showing you first and asking:",
        );
        println!("    1. choose the web server (nginx, Apache or Caddy)");
        println!("    2. the name it answers to, and the DNS record that points it here");
        println!("    3. its configuration, written, checked, and loaded");
        println!("    4. a certificate, from Let's Encrypt");
        println!("    5. Breeze Core moved behind it");
        say(&format!(
            "Every change is recorded, and `breeze-core proxy --undo` takes all of it back out. \
             {}",
            if ctx.dry_run {
                "This is a dry run: everything is shown, nothing is changed."
            } else {
                ""
            }
        ));

        // --- Breeze Core as it is -------------------------------------------------
        heading("Breeze Core, as it is now");
        let env = read_env();
        let port = env_value(&env, "BREEZE_PORT").unwrap_or_else(|| "8420".into());
        let host = env_value(&env, "BREEZE_HOST").unwrap_or_else(|| "127.0.0.1".into());
        match http_get(
            &host,
            port.parse().unwrap_or(8420),
            "localhost",
            "/api/health",
        ) {
            Some(status) => say(&format!(
                "Running, on {host}:{port} (health check answered {status})."
            )),
            None => say(&format!(
                "Configured for {host}:{port}, but not answering there right now. That is fine \
                 for setting this up; start it before testing."
            )),
        }

        // --- 1. the server ---------------------------------------------------------
        let kind = choose_server(&mut ctx, options.server.as_deref())?;
        let Some(kind) = kind else { return Ok(1) };
        ctx.journal.server = Some(kind);

        // --- 2. the name ------------------------------------------------------------
        let domain = ask_domain(options.domain.as_deref())?;
        ctx.journal.domain = domain.clone();
        if !dns(&domain)? {
            return Ok(1);
        }

        // --- 3. the configuration --------------------------------------------------
        let Some(_) = configure(&mut ctx, kind, &domain, &port)? else {
            save_journal(&ctx)?;
            return Ok(1);
        };

        // --- 4. the certificate ------------------------------------------------------
        let tls = certificate(&mut ctx, kind, &domain)?;

        // --- 5. Breeze Core behind it ---------------------------------------------
        behind_proxy(&mut ctx, &env)?;
        save_journal(&ctx)?;

        // --- and now ----------------------------------------------------------------
        heading("Checking");
        if ctx.dry_run {
            say("(dry run: nothing was set up, so there is nothing to check)");
        } else {
            check(&domain, &port, tls);
        }
        heading("Done");
        say(&format!(
            "Breeze Core is at https://{domain}/ once the certificate is in place. Point the app \
             and the web panel there. Undo all of this with: sudo breeze-core proxy --undo"
        ));
        say(
            "One more check is worth doing from outside: on a phone with Wi-Fi off, open \
             https://<the name>/api/system in the app's Nerd screen and make sure client_ip is \
             your phone's public address, not 127.0.0.1. If it says 127.0.0.1, the LAN-only \
             protection is not working, so stop and see the wiki page.",
        );
        Ok(0)
    }

    // --- 1. which server ---------------------------------------------------------

    fn installed(kind: Kind) -> Option<String> {
        let (bin, args): (&str, &[&str]) = match kind {
            Kind::Nginx => ("nginx", &["-v"]),
            Kind::Apache if on_path("apache2") => ("apache2", &["-v"]),
            Kind::Apache => ("httpd", &["-v"]),
            Kind::Caddy => ("caddy", &["version"]),
        };
        let out = Command::new(bin).args(args).output().ok()?;
        // nginx prints its version on stderr.
        let text = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        text.lines().next().map(|l| l.trim().to_string())
    }

    fn choose_server(ctx: &mut Ctx, asked: Option<&str>) -> Result<Option<Kind>, String> {
        heading("1. The web server");
        let found: Vec<(Kind, String)> = [Kind::Caddy, Kind::Nginx, Kind::Apache]
            .into_iter()
            .filter_map(|k| installed(k).map(|v| (k, v)))
            .collect();
        for (kind, version) in &found {
            println!("  {:<8} installed  ({version})", kind.name());
        }
        for port in [80, 443] {
            if TcpStream::connect_timeout(
                &format!("127.0.0.1:{port}").parse().unwrap(),
                Duration::from_millis(500),
            )
            .is_ok()
            {
                println!("  something is already answering on port {port}");
            }
        }

        let kind = if let Some(name) = asked {
            match name.to_ascii_lowercase().as_str() {
                "nginx" => Kind::Nginx,
                "apache" | "httpd" | "apache2" => Kind::Apache,
                "caddy" => Kind::Caddy,
                other => {
                    return Err(format!(
                        "unknown --server '{other}': nginx, apache or caddy"
                    ))
                }
            }
        } else if found.len() == 1 {
            say(&format!(
                "{} is already installed, so I'll use that.",
                found[0].0.name()
            ));
            found[0].0
        } else if found.len() > 1 {
            say(
                "More than one is installed. Use the one already serving your other sites, if any:",
            );
            for (i, (kind, _)) in found.iter().enumerate() {
                println!("  {}  {}", i + 1, kind.name());
            }
            loop {
                let typed = ask("Choose: ")?.ok_or("no answer")?;
                if let Some(k) = typed
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| found.get(n.wrapping_sub(1)))
                {
                    break k.0;
                }
            }
        } else {
            say(
                "None is installed. I'd suggest Caddy: it fetches and renews its own certificate, \
                 so there is one moving part fewer. nginx is the other good choice.",
            );
            println!("  1  Caddy (recommended)\n  2  nginx\n  3  Apache");
            let kind = loop {
                match ask("Choose 1-3: ")?.as_deref() {
                    Some("1") => break Kind::Caddy,
                    Some("2") => break Kind::Nginx,
                    Some("3") => break Kind::Apache,
                    Some(_) => println!("  1, 2 or 3, please."),
                    None => return Err("no answer".into()),
                }
            };
            let Some(pm) = ctx.pm else {
                say(&format!(
                    "I can't tell which package manager this system uses. Install {} yourself, \
                     then run this again.",
                    kind.name()
                ));
                return Ok(None);
            };
            let Some(packages) = pm.server_packages(kind) else {
                say(
                    "This wizard sets Apache up on Debian- and Fedora-style systems only. nginx or \
                     Caddy will work here.",
                );
                return Ok(None);
            };
            if !yes_no(&format!("Install {} now?", kind.name()), true)? {
                return Ok(None);
            }
            run_cmd(ctx, &pm.install(&packages), true)?;
            kind
        };
        if kind == Kind::Apache
            && !(Path::new("/etc/apache2").is_dir() || Path::new("/etc/httpd/conf.d").is_dir())
        {
            say("This wizard sets Apache up on Debian- and Fedora-style layouts (/etc/apache2 or /etc/httpd/conf.d) only.");
            return Ok(None);
        }
        Ok(Some(kind))
    }

    // --- 2. the name and its DNS ---------------------------------------------------

    fn valid_domain(name: &str) -> bool {
        let labels: Vec<&str> = name.split('.').collect();
        name.len() <= 253
            && labels.len() >= 2
            && labels.iter().all(|l| {
                !l.is_empty()
                    && l.len() <= 63
                    && !l.starts_with('-')
                    && !l.ends_with('-')
                    && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
    }

    fn ask_domain(asked: Option<&str>) -> Result<String, String> {
        heading("2. The name it answers to");
        if let Some(d) = asked {
            let d = d.trim().trim_end_matches('.').to_ascii_lowercase();
            return if valid_domain(&d) {
                Ok(d)
            } else {
                Err(format!("'{d}' is not a hostname"))
            };
        }
        say(
            "The full name people will type, such as breeze.example.com: a name under a domain \
             you control, or one a dynamic-DNS service gave you.",
        );
        loop {
            let typed = ask("Name: ")?.ok_or("no name given")?;
            let d = typed.trim().trim_end_matches('.').to_ascii_lowercase();
            if valid_domain(&d) {
                return Ok(d);
            }
            println!("  That isn't a hostname. Something like breeze.example.com, please.");
        }
    }

    /// Explain the record, then check it, as many times as the person likes.
    fn dns(domain: &str) -> Result<bool, String> {
        let lan = breeze_device::scan::local_ipv4();
        say(&format!(
            "For {domain} to reach this machine, two things have to be true, and both are set \
             up outside this machine:"
        ));
        say(
            "  The DNS record. At whoever hosts your domain, add a record for this name: \
             either a CNAME pointing it at a name that already points at your home (your \
             dynamic-DNS name, say), or an A record holding your home's public address.",
        );
        say(&format!(
            "  The router. Forward ports 80 and 443 to this machine{}. Port 80 is only needed \
             for getting and renewing the certificate.",
            lan.map(|ip| format!(" ({ip})")).unwrap_or_default()
        ));
        let public = if yes_no(
            "\nShall I look up your public address, to check the record? It asks api.ipify.org, \
             which sees only that a request came from your address.",
            true,
        )? {
            public_ip()
        } else {
            None
        };
        loop {
            let found: Vec<IpAddr> = (domain, 443)
                .to_socket_addrs()
                .map(|a| a.map(|s| s.ip()).collect())
                .unwrap_or_default();
            if found.is_empty() {
                println!("  {domain} does not resolve yet.");
            } else {
                let list: Vec<String> = found.iter().map(ToString::to_string).collect();
                println!("  {domain} resolves to {}", list.join(", "));
            }
            match public {
                Some(ip) if found.contains(&ip) => {
                    say(&format!(
                        "That is your public address ({ip}). The record is right."
                    ));
                    return Ok(true);
                }
                Some(ip) if !found.is_empty() => say(&format!(
                    "That is not your public address, which is {ip}. If you use Cloudflare's \
                     proxy (the orange cloud), that is expected; otherwise the record points \
                     somewhere else."
                )),
                Some(ip) => say(&format!(
                    "Your public address is {ip}; the record should point there."
                )),
                None if !found.is_empty() => say("(Not compared with your public address.)"),
                None => {}
            }
            say("DNS changes can take a few minutes to show.");
            match ask("[r]etry, [c]ontinue anyway, or [q]uit? ")?
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("r") | Some("retry") | Some("") => continue,
                Some("c") | Some("continue") => return Ok(true),
                _ => return Ok(false),
            }
        }
    }

    fn public_ip() -> Option<IpAddr> {
        ureq::get("https://api.ipify.org")
            .timeout(Duration::from_secs(10))
            .call()
            .ok()?
            .into_string()
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    // --- 3. the configuration ------------------------------------------------------

    /// Where the file goes, and what to run to switch it on, for this layout.
    fn target(kind: Kind) -> (String, Vec<Vec<String>>) {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        match kind {
            // Alpine renamed conf.d to http.d; everyone else's nginx.conf
            // includes conf.d/*.conf, Debian's included.
            Kind::Nginx if Path::new("/etc/nginx/http.d").is_dir() => {
                ("/etc/nginx/http.d/breeze-core.conf".into(), vec![])
            }
            Kind::Nginx => ("/etc/nginx/conf.d/breeze-core.conf".into(), vec![]),
            Kind::Apache if Path::new("/etc/apache2").is_dir() => (
                "/etc/apache2/sites-available/breeze-core.conf".into(),
                vec![
                    s(&["a2enmod", "-q", "proxy", "proxy_http", "headers"]),
                    s(&["a2ensite", "-q", "breeze-core"]),
                ],
            ),
            Kind::Apache => ("/etc/httpd/conf.d/breeze-core.conf".into(), vec![]),
            Kind::Caddy => ("/etc/caddy/Caddyfile".into(), vec![]),
        }
    }

    fn configure(
        ctx: &mut Ctx,
        kind: Kind,
        domain: &str,
        port: &str,
    ) -> Result<Option<()>, String> {
        heading(&format!("3. {}'s configuration", kind.name()));
        let (path, enable) = target(kind);
        let text = match kind {
            Kind::Nginx => nginx_conf(domain, port),
            Kind::Apache => apache_conf(domain, port),
            Kind::Caddy => caddy_block(domain, port),
        };
        let full = if kind == Kind::Caddy {
            // The Caddyfile is one file for every site, so this is added to it,
            // between markers, rather than replacing it.
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            if existing.contains(MARK_START) {
                return Err(format!(
                    "{path} already has a Breeze Core block; remove it, or run --undo"
                ));
            }
            say(&format!(
                "This is added to the end of {path}, which keeps everything already in it:"
            ));
            format!(
                "{}{}{text}",
                existing,
                if existing.is_empty() || existing.ends_with('\n') {
                    ""
                } else {
                    "\n"
                }
            )
        } else {
            say(&format!("This goes in {path}:"));
            text.clone()
        };
        println!();
        for line in text.lines() {
            println!("    | {line}");
        }
        println!();
        say(
            "In short: requests for the name go to Breeze Core on this machine; the address \
             it reports is the real one and cannot be forged by the client; the live updates \
             are passed through unbuffered; and pairing approval answers the local network only.",
        );
        if !yes_no("Write it?", true)? {
            return Ok(None);
        }
        write_file(ctx, &path, &full)?;
        for command in enable {
            let undo: Vec<String> = match command.first().map(String::as_str) {
                Some("a2ensite") => vec!["a2dissite".into(), "-q".into(), "breeze-core".into()],
                _ => vec![],
            };
            run_cmd(ctx, &command, true)?;
            if !undo.is_empty() {
                ctx.journal.steps.push(Step::Ran { command, undo });
            }
        }

        // Checked before anything is reloaded: a broken file never goes live.
        say("Checking the configuration before loading it:");
        if !validate(ctx, kind)? {
            say("The check failed (above), so nothing has been reloaded. Taking the change back out.");
            rollback(ctx)?;
            return Ok(None);
        }
        reload(ctx, kind)?;
        Ok(Some(()))
    }

    const MARK_START: &str = "# --- breeze-core (added by `breeze-core proxy`) ---";
    const MARK_END: &str = "# --- end breeze-core ---";

    /// The private ranges the server itself treats as "the LAN", so the proxy and
    /// the server agree about who may approve a pairing.
    const LAN: [&str; 5] = [
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "127.0.0.1",
        "::1",
    ];

    fn nginx_conf(domain: &str, port: &str) -> String {
        let allow: String = LAN
            .iter()
            .map(|r| format!("        allow {r};\n"))
            .collect();
        let headers = "        proxy_set_header Host              $host;\n        \
                       proxy_set_header X-Forwarded-For   $remote_addr;\n        \
                       proxy_set_header X-Forwarded-Proto $scheme;\n";
        format!(
            "# Breeze Core behind nginx, written by `breeze-core proxy`; undo with\n\
             # `breeze-core proxy --undo`. certbot adds the HTTPS half to this block.\n\
             server {{\n    listen 80;\n    listen [::]:80;\n    server_name {domain};\n\n    \
             # $remote_addr, NOT $proxy_add_x_forwarded_for: overwrite, never append.\n\n    \
             # Pairing approval and device management: the local network only.\n    \
             location ~ ^/api/auth/(enroll/approve|devices) {{\n{allow}        deny all;\n        \
             proxy_pass http://127.0.0.1:{port};\n{headers}    }}\n\n    \
             # The live updates: one response that never ends. Never buffered.\n    \
             location = /api/units/stream {{\n        proxy_pass http://127.0.0.1:{port};\n{headers}        \
             proxy_buffering off;\n        proxy_cache off;\n        proxy_read_timeout 1h;\n        gzip off;\n    }}\n\n    \
             location / {{\n        proxy_pass http://127.0.0.1:{port};\n{headers}    }}\n}}\n"
        )
    }

    fn apache_conf(domain: &str, port: &str) -> String {
        format!(
            "# Breeze Core behind Apache, written by `breeze-core proxy`; undo with\n\
             # `breeze-core proxy --undo`. certbot adds the HTTPS half alongside this.\n\
             <VirtualHost *:80>\n    ServerName {domain}\n    ProxyPreserveHost On\n\n    \
             # X-Forwarded-For OVERWRITTEN with the real peer: mod_proxy's own header\n    \
             # appends to whatever the client sent, so it is off and the header is set.\n    \
             ProxyAddHeaders Off\n    RequestHeader set X-Forwarded-For   \"expr=%{{REMOTE_ADDR}}\"\n    \
             RequestHeader set X-Forwarded-Proto \"expr=%{{REQUEST_SCHEME}}\"\n\n    \
             # Pairing approval and device management: the local network only.\n    \
             <Location ~ \"^/api/auth/(enroll/approve|devices)\">\n        Require ip {}\n    </Location>\n\n    \
             # The live updates: never buffered or compressed.\n    \
             <Location \"/api/units/stream\">\n        SetEnv no-gzip 1\n        SetEnv proxy-sendchunked 1\n    </Location>\n\n    \
             ProxyPass        / http://127.0.0.1:{port}/\n    ProxyPassReverse / http://127.0.0.1:{port}/\n\
             </VirtualHost>\n",
            LAN.join(" ")
        )
    }

    fn caddy_block(domain: &str, port: &str) -> String {
        let up = format!(
            "\t\treverse_proxy 127.0.0.1:{port} {{\n\t\t\theader_up X-Forwarded-For {{remote_host}}\n\t\t\t\
             header_up X-Forwarded-Proto {{scheme}}\n"
        );
        format!(
            "{MARK_START}\n# Undo with `breeze-core proxy --undo`. No trusted_proxies anywhere: that is what\n\
             # makes Caddy overwrite X-Forwarded-For with the real peer.\n\
             {domain} {{\n\tencode gzip\n\theader {{\n\t\tStrict-Transport-Security \"max-age=63072000; includeSubDomains\"\n\t\t-Server\n\t}}\n\n\t\
             # Pairing approval and device management: the local network only.\n\t\
             @admin path /api/auth/enroll/approve* /api/auth/devices*\n\thandle @admin {{\n\t\t\
             @notlan not remote_ip {}\n\t\trespond @notlan 403\n{up}\t\t}}\n\t}}\n\n\t\
             # The live updates: never buffered.\n\t@stream path /api/units/stream\n\thandle @stream {{\n{up}\t\t\t\
             flush_interval -1\n\t\t}}\n\t}}\n\n\thandle {{\n{up}\t\t}}\n\t}}\n}}\n{MARK_END}\n",
            ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "127.0.0.1/8", "::1/128"].join(" ")
        )
    }

    fn validate(ctx: &Ctx, kind: Kind) -> Result<bool, String> {
        let command: Vec<String> = match kind {
            Kind::Nginx => vec!["nginx".into(), "-t".into()],
            Kind::Apache if on_path("apache2ctl") => vec!["apache2ctl".into(), "configtest".into()],
            Kind::Apache => vec!["apachectl".into(), "configtest".into()],
            Kind::Caddy => [
                "caddy",
                "validate",
                "--config",
                "/etc/caddy/Caddyfile",
                "--adapter",
                "caddyfile",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        };
        run_cmd(ctx, &command, false)
    }

    fn service_name(kind: Kind) -> &'static str {
        match kind {
            Kind::Nginx => "nginx",
            Kind::Apache if Path::new("/etc/apache2").is_dir() => "apache2",
            Kind::Apache => "httpd",
            Kind::Caddy => "caddy",
        }
    }

    /// Reload, or start it if it was not running -- through the init system.
    fn reload(ctx: &Ctx, kind: Kind) -> Result<(), String> {
        let service = service_name(kind);
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        if Path::new("/run/systemd/system").is_dir() {
            run_cmd(ctx, &s(&["systemctl", "enable", "--now", service]), true)?;
            run_cmd(ctx, &s(&["systemctl", "reload", service]), true)?;
        } else if Path::new("/run/openrc").exists() {
            run_cmd(ctx, &s(&["rc-update", "add", service, "default"]), false)?;
            if !run_cmd(ctx, &s(&["rc-service", service, "reload"]), false)? {
                run_cmd(ctx, &s(&["rc-service", service, "start"]), true)?;
            }
        } else {
            // No init system to ask (a container): the server's own controls.
            let (reload, start): (Vec<String>, Vec<String>) = match kind {
                Kind::Nginx => (s(&["nginx", "-s", "reload"]), s(&["nginx"])),
                Kind::Apache => {
                    let ctl = if on_path("apache2ctl") {
                        "apache2ctl"
                    } else {
                        "apachectl"
                    };
                    (s(&[ctl, "-k", "graceful"]), s(&[ctl, "-k", "start"]))
                }
                Kind::Caddy => (
                    s(&[
                        "caddy",
                        "reload",
                        "--config",
                        "/etc/caddy/Caddyfile",
                        "--adapter",
                        "caddyfile",
                    ]),
                    s(&[
                        "caddy",
                        "start",
                        "--config",
                        "/etc/caddy/Caddyfile",
                        "--adapter",
                        "caddyfile",
                    ]),
                ),
            };
            if !run_cmd(ctx, &reload, false)? {
                run_cmd(ctx, &start, true)?;
            }
        }
        Ok(())
    }

    // --- 4. the certificate --------------------------------------------------------

    /// True when HTTPS is (or will be) set up.
    fn certificate(ctx: &mut Ctx, kind: Kind, domain: &str) -> Result<bool, String> {
        heading("4. The certificate");
        if kind == Kind::Caddy {
            say(&format!(
                "Caddy gets the certificate for {domain} itself, from Let's Encrypt, the first \
                 time it is asked for the name, and renews it on its own. It needs the DNS \
                 record and port 80 forwarded; if it cannot get one yet, it keeps trying."
            ));
            return Ok(true);
        }
        say(&format!(
            "certbot gets a free certificate for {domain} from Let's Encrypt, adds the HTTPS half \
             to the configuration just written, redirects plain HTTP to HTTPS, and renews the \
             certificate on its own before it expires."
        ));
        say(
            "certbot will ask for an email address (for expiry warnings) and for you to agree \
             to Let's Encrypt's terms of service. Those questions are certbot's and the answers \
             are yours; I only start it.",
        );
        if !on_path("certbot") {
            let Some(pm) = ctx.pm else {
                say("certbot is not installed, and I can't tell how to install it here. Install it, then run: certbot --nginx (or --apache)");
                return Ok(false);
            };
            if !yes_no("certbot is not installed. Install it now?", true)? {
                say("Skipping the certificate. Run certbot later; until then this serves plain HTTP only.");
                return Ok(false);
            }
            run_cmd(ctx, &pm.install(&pm.certbot_packages(kind)), true)?;
        }
        if !yes_no("Start certbot now?", true)? {
            say(&format!(
                "Skipped. When you are ready: sudo certbot --{} -d {domain} --redirect --hsts",
                if kind == Kind::Nginx {
                    "nginx"
                } else {
                    "apache"
                }
            ));
            return Ok(false);
        }
        let plugin = if kind == Kind::Nginx {
            "--nginx"
        } else {
            "--apache"
        };
        let ok = run_cmd(
            ctx,
            &["certbot", plugin, "-d", domain, "--redirect", "--hsts"].map(String::from),
            false,
        )?;
        if !ok {
            say(
                "certbot did not get a certificate (its own explanation is above). The usual \
                 causes are a DNS record that does not point here yet, or port 80 not forwarded. \
                 The site still works over plain HTTP meanwhile; run the certbot line again once \
                 that is fixed.",
            );
        }
        Ok(ok)
    }

    // --- 5. Breeze Core behind it --------------------------------------------------

    fn behind_proxy(ctx: &mut Ctx, env: &str) -> Result<(), String> {
        heading("5. Breeze Core, behind the proxy");
        say(
            "Two settings change in Breeze Core's own configuration. BREEZE_HOST becomes \
             127.0.0.1, so it answers only the proxy on this machine; and --behind-proxy \
             tells it to read the real client address from the proxy rather than seeing \
             everyone as 127.0.0.1 -- without it, pairing approval would trust the whole \
             internet as 'local'.",
        );
        say(
            "That means the app and the panel use https://<the name> from now on, at home too. \
             Most routers handle that (it is called NAT loopback or hairpinning).",
        );
        let keep_lan = !yes_no(
            "Make Breeze Core answer only through the proxy? (recommended)",
            true,
        )?;
        if keep_lan {
            say(
                "Keeping the direct address as well. Then make sure your router does NOT \
                 forward Breeze Core's own port (8420): with --behind-proxy on, anyone who can \
                 reach that port directly can claim to be on your network.",
            );
        }
        let updated = edit_env(env, keep_lan);
        let path = env_path();
        for (before, after) in env.lines().zip(updated.lines()).filter(|(a, b)| a != b) {
            println!("    - {before}\n    + {after}");
        }
        if updated.lines().count() > env.lines().count() {
            for added in updated.lines().skip(env.lines().count()) {
                println!("    + {added}");
            }
        }
        if !yes_no(
            &format!("Change {path} like that and restart Breeze Core?"),
            true,
        )? {
            say("Left as it was. Without --behind-proxy the LAN-only checks see only the proxy.");
            return Ok(());
        }
        write_file(ctx, &path, &updated)?;
        restart_breeze(ctx)?;
        Ok(())
    }

    /// BREEZE_HOST to loopback (unless keeping the LAN bind), and --behind-proxy
    /// into BREEZE_OPTS, keeping every other line and comment as it was.
    fn edit_env(env: &str, keep_lan: bool) -> String {
        let mut out = Vec::new();
        let (mut saw_host, mut saw_opts) = (false, false);
        for line in env.lines() {
            if line.starts_with("BREEZE_HOST=") {
                saw_host = true;
                out.push(if keep_lan {
                    line.to_string()
                } else {
                    "BREEZE_HOST=127.0.0.1".into()
                });
            } else if let Some(rest) = line.strip_prefix("BREEZE_OPTS=") {
                saw_opts = true;
                let opts = rest.trim().trim_matches('"');
                out.push(if opts.split_whitespace().any(|o| o == "--behind-proxy") {
                    line.to_string()
                } else if opts.is_empty() {
                    "BREEZE_OPTS=--behind-proxy".into()
                } else {
                    format!("BREEZE_OPTS=\"{opts} --behind-proxy\"")
                });
            } else {
                out.push(line.to_string());
            }
        }
        if !saw_host && !keep_lan {
            out.push("BREEZE_HOST=127.0.0.1".into());
        }
        if !saw_opts {
            out.push("BREEZE_OPTS=--behind-proxy".into());
        }
        out.join("\n") + "\n"
    }

    fn restart_breeze(ctx: &Ctx) -> Result<(), String> {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        if Path::new("/run/systemd/system").is_dir() {
            run_cmd(ctx, &s(&["systemctl", "try-restart", "breeze-core"]), false)?;
        } else if Path::new("/run/openrc").exists() {
            run_cmd(ctx, &s(&["rc-service", "breeze-core", "restart"]), false)?;
        } else {
            say("Restart Breeze Core with your init system for the change to take effect.");
        }
        Ok(())
    }

    fn check(domain: &str, port: &str, tls: bool) {
        match http_get("127.0.0.1", 80, domain, "/api/health") {
            Some(status) => say(&format!(
                "The proxy answers for {domain} on this machine (HTTP {status})."
            )),
            None => say("The proxy did not answer on port 80 here. Its log will say why."),
        }
        match http_get(
            "127.0.0.1",
            port.parse().unwrap_or(8420),
            "localhost",
            "/api/health",
        ) {
            Some(_) => say("Breeze Core answers behind it."),
            None => say("Breeze Core is not answering on 127.0.0.1 yet. Is it running?"),
        }
        if tls {
            let url = format!("https://{domain}/api/health");
            match ureq::get(&url).timeout(Duration::from_secs(10)).call() {
                Ok(r) => say(&format!(
                    "{url} answers ({}), certificate and all.",
                    r.status()
                )),
                Err(e) => say(&format!(
                    "{url} did not answer from here ({e}). From inside your network that can \
                     just be the router not doing NAT loopback; try it from a phone on mobile \
                     data before worrying."
                )),
            }
        }
    }

    // --- undo ------------------------------------------------------------------------

    fn save_journal(ctx: &Ctx) -> Result<(), String> {
        if ctx.dry_run || ctx.journal.steps.is_empty() {
            return Ok(());
        }
        let text = serde_json::to_string_pretty(&ctx.journal).map_err(|e| e.to_string())?;
        std::fs::write(journal_path(), text + "\n")
            .map_err(|e| format!("cannot write {}: {e}", journal_path()))?;
        say(&format!("Every change is recorded in {}.", journal_path()));
        Ok(())
    }

    /// Take back the changes of a run that stopped partway, in reverse.
    fn rollback(ctx: &mut Ctx) -> Result<(), String> {
        let steps: Vec<Step> = ctx.journal.steps.drain(..).rev().collect();
        for step in steps {
            reverse(ctx, &step)?;
        }
        Ok(())
    }

    fn reverse(ctx: &Ctx, step: &Step) -> Result<(), String> {
        match step {
            Step::Created { path } => {
                println!("  remove {path}");
                if !ctx.dry_run {
                    std::fs::remove_file(path)
                        .or_else(not_found_ok)
                        .map_err(|e| format!("cannot remove {path}: {e}"))?;
                }
            }
            Step::Replaced { path, backup } => {
                println!("  restore {path} from {backup}");
                if !ctx.dry_run {
                    std::fs::copy(backup, path)
                        .map_err(|e| format!("cannot restore {path}: {e}"))?;
                    std::fs::remove_file(backup)
                        .or_else(not_found_ok)
                        .map_err(|e| e.to_string())?;
                }
            }
            Step::Ran { undo, .. } => {
                run_cmd(ctx, undo, false)?;
            }
        }
        Ok(())
    }

    fn not_found_ok(e: std::io::Error) -> std::io::Result<()> {
        if e.kind() == std::io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(e)
        }
    }

    fn undo(dry_run: bool) -> Result<i32, String> {
        let path = journal_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => {
                say(&format!("There is nothing to undo: no journal at {path}."));
                return Ok(0);
            }
        };
        let journal: Journal =
            serde_json::from_str(&text).map_err(|e| format!("{path} is unreadable: {e}"))?;
        if !dry_run && !is_root() {
            return Err("undoing changes system configuration, so it needs root: sudo breeze-core proxy --undo".into());
        }
        heading(&format!(
            "Undoing the set-up for {} ({})",
            journal.domain,
            journal.server.map(Kind::name).unwrap_or("?")
        ));
        say("These changes, newest first:");
        for step in journal.steps.iter().rev() {
            match step {
                Step::Created { path } => println!("  remove {path}"),
                Step::Replaced { path, .. } => println!("  put back the previous {path}"),
                Step::Ran { undo, .. } => println!("  run: {}", undo.join(" ")),
            }
        }
        let ctx = Ctx {
            dry_run,
            journal: Journal::default(),
            pm: None,
        };
        if !yes_no("Go ahead?", true)? {
            return Ok(1);
        }
        for step in journal.steps.iter().rev() {
            reverse(&ctx, step)?;
        }
        if let Some(kind) = journal.server {
            if validate(&ctx, kind)? {
                reload(&ctx, kind)?;
            }
        }
        restart_breeze(&ctx)?;
        if !dry_run {
            std::fs::remove_file(&path).map_err(|e| format!("cannot remove {path}: {e}"))?;
        }
        say(&format!(
            "Done. A certificate, if one was issued, stays in /etc/letsencrypt; remove it with: \
             sudo certbot delete --cert-name {}",
            journal.domain
        ));
        Ok(0)
    }

    // --- doing ---------------------------------------------------------------------

    /// Write a file, backing up whatever it replaces, and record it.
    fn write_file(ctx: &mut Ctx, path: &str, text: &str) -> Result<(), String> {
        let exists = Path::new(path).exists();
        if ctx.dry_run {
            println!(
                "  (dry run) would {} {path}",
                if exists { "replace" } else { "create" }
            );
            return Ok(());
        }
        if exists {
            let backup = format!("{path}.before-breeze-proxy");
            std::fs::copy(path, &backup).map_err(|e| format!("cannot back up {path}: {e}"))?;
            println!("  backed up {path} to {backup}");
            ctx.journal.steps.push(Step::Replaced {
                path: path.to_string(),
                backup,
            });
        } else {
            ctx.journal.steps.push(Step::Created {
                path: path.to_string(),
            });
        }
        if let Some(dir) = Path::new(path).parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        std::fs::write(path, text).map_err(|e| format!("cannot write {path}: {e}"))?;
        println!("  wrote {path}");
        Ok(())
    }

    /// Run a command, showing it first. Returns whether it succeeded; with
    /// `required`, a failure is an error.
    fn run_cmd(ctx: &Ctx, command: &[String], required: bool) -> Result<bool, String> {
        println!("  $ {}", command.join(" "));
        if ctx.dry_run {
            return Ok(true);
        }
        let status = Command::new(&command[0])
            .args(&command[1..])
            .status()
            .map_err(|e| format!("cannot run {}: {e}", command[0]))?;
        if !status.success() && required {
            return Err(format!("`{}` failed ({status})", command.join(" ")));
        }
        Ok(status.success())
    }

    // --- small things ----------------------------------------------------------------

    fn is_root() -> bool {
        Command::new("id")
            .arg("-u")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
            .unwrap_or(false)
    }

    fn on_path(bin: &str) -> bool {
        std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).any(|d| d.join(bin).is_file()))
            .unwrap_or(false)
    }

    fn read_env() -> String {
        std::fs::read_to_string(env_path()).unwrap_or_default()
    }

    fn env_value(env: &str, key: &str) -> Option<String> {
        env.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .map(|v| v.trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
    }

    /// A bare HTTP/1.1 GET with a chosen Host header; the status, or None.
    fn http_get(ip: &str, port: u16, host: &str, path: &str) -> Option<u16> {
        let addr = format!("{ip}:{port}").parse().ok()?;
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        )
        .ok()?;
        let mut head = [0u8; 64];
        let n = stream.read(&mut head).ok()?;
        let line = String::from_utf8_lossy(&head[..n]);
        line.split_whitespace().nth(1)?.parse().ok()
    }

    fn heading(title: &str) {
        println!("\n== {title} ==");
    }

    fn say(text: &str) {
        let after_newlines = text.trim_start_matches('\n');
        for _ in 0..(text.len() - after_newlines.len()) {
            println!();
        }
        let body = after_newlines.trim_start_matches(' ');
        let pad = " ".repeat((after_newlines.len() - body.len()).max(2));
        let mut line = String::new();
        for word in body.split_whitespace() {
            if !line.is_empty() && pad.len() + line.len() + 1 + word.len() > 78 {
                println!("{pad}{line}");
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            println!("{pad}{line}");
        }
    }

    fn ask(message: &str) -> Result<Option<String>, String> {
        print!("{message}");
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(line.trim().to_string())),
            Err(e) => Err(format!("cannot read from the terminal: {e}")),
        }
    }

    fn yes_no(question: &str, default: bool) -> Result<bool, String> {
        let hint = if default { "[Y/n]" } else { "[y/N]" };
        loop {
            match ask(&format!("{question} {hint} "))?
                .as_deref()
                .map(str::to_ascii_lowercase)
            {
                None => return Ok(default),
                Some(a) if a.is_empty() => return Ok(default),
                Some(a) if a == "y" || a == "yes" => return Ok(true),
                Some(a) if a == "n" || a == "no" => return Ok(false),
                Some(_) => println!("  y or n, please."),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn hostnames_are_checked_before_they_reach_a_config_file() {
            assert!(valid_domain("breeze.example.com"));
            assert!(valid_domain("ac-1.home.example.org"));
            for bad in [
                "localhost",
                "breeze",
                "-x.example.com",
                "a..b",
                "a b.com",
                "x.example.com;",
                "$(id).example.com",
            ] {
                assert!(!valid_domain(bad), "{bad}");
            }
        }

        #[test]
        fn every_config_overwrites_the_forwarded_header() {
            let n = nginx_conf("b.example.com", "8420");
            assert!(n.contains("X-Forwarded-For   $remote_addr;"));
            assert!(!n.contains("proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for"));
            let a = apache_conf("b.example.com", "8420");
            assert!(a.contains("ProxyAddHeaders Off"));
            assert!(a.contains("X-Forwarded-For   \"expr=%{REMOTE_ADDR}\""));
            // Not as a directive: the comment saying so is allowed to name it.
            let c = caddy_block("b.example.com", "8420");
            assert!(!c
                .lines()
                .any(|l| !l.trim_start().starts_with('#') && l.contains("trusted_proxies")));
        }

        #[test]
        fn the_admin_paths_are_the_real_ones() {
            // A block written against /api/auth/approve protects nothing.
            for text in [
                nginx_conf("b.example.com", "8420"),
                apache_conf("b.example.com", "8420"),
            ] {
                assert!(
                    text.contains("^/api/auth/(enroll/approve|devices)"),
                    "{text}"
                );
            }
            assert!(caddy_block("b.example.com", "8420").contains("/api/auth/enroll/approve*"));
        }

        #[test]
        fn the_env_edit_keeps_everything_else() {
            let before = "# comment\nBREEZE_HOST=192.168.1.10\nBREEZE_PORT=8420\nBREEZE_OPTS=\n#BREEZE_WORKERS=8\n";
            let after = edit_env(before, false);
            assert_eq!(
                after,
                "# comment\nBREEZE_HOST=127.0.0.1\nBREEZE_PORT=8420\nBREEZE_OPTS=--behind-proxy\n#BREEZE_WORKERS=8\n"
            );
            // Idempotent, and an existing option is kept.
            assert_eq!(edit_env(&after, false), after);
            let other = edit_env("BREEZE_OPTS=--foo\n", true);
            assert_eq!(other, "BREEZE_OPTS=\"--foo --behind-proxy\"\n");
        }
    }
}
