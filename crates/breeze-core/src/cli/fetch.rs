//! `breeze-core fetch` -- a V3 unit's token and key, from the Midea account
//! the unit is paired with, explained as it goes.
//!
//! The situation it explains, as of October 2026: a V3 unit answers nothing
//! without a token and key, and only Midea issues them -- now only to the
//! account the unit is paired with, on MSmartHome (outside China) or Meiju
//! (in it). NetHome Plus, which many units came with, still logs in but has
//! refused every token since August 2026. So a NetHome Plus user has to move
//! the unit to MSmartHome first, and this says how, rather than leaving them
//! with an error code.
//!
//! What it promises, and keeps:
//!
//! * **The password goes only to a cloud that has the account.** Which clouds
//!   know it is asked first, through a lookup that takes no password.
//! * **The password is never stored or shown.** It is read without echo and
//!   dropped after the one login ([`breeze_cloud::Credentials`] scrubs it).
//! * **A token is checked on the unit before it is saved**: a V3 handshake and
//!   a state read, which change nothing on the unit.
//! * **config.json is backed up first**, to `config.json.before-fetch`.

/// What the command line asked for. Never a password: that is read later,
/// without echo, and not kept.
#[derive(Debug)]
pub struct Options {
    /// Units by name, address or id; empty means every unit with no token.
    pub units: Vec<String>,
    pub config: Option<String>,
    /// `smarthome`, `meiju` or `nethome`, to skip the question.
    pub cloud: Option<String>,
    pub account: Option<String>,
    /// Read the password as one line of standard input, for scripts.
    pub password_stdin: bool,
}

/// A build without TLS -- the router build -- cannot reach the cloud at all.
#[cfg(not(feature = "cloud"))]
pub fn run(_options: Options) -> Result<i32, String> {
    Err(
        "this build of breeze-core has no TLS (it was built without the `cloud` \
         feature, as the small router builds are), so it cannot reach Midea's \
         cloud. Run `breeze-core fetch` from a full build on another machine and \
         copy the token and key across, or add the unit with a token and key you \
         already hold: `breeze-core pair --ip ADDRESS`."
            .into(),
    )
}

#[cfg(feature = "cloud")]
pub fn run(options: Options) -> Result<i32, String> {
    imp::run(options)
}

#[cfg(feature = "cloud")]
mod imp {
    use std::io::{BufRead, Write};
    use std::time::Duration;

    use breeze_cloud::{Cloud, CloudError, Credentials, Session, Token};
    use breeze_proto::discover::Version;
    use breeze_store::models::{AppConfig, UnitConfig};
    use breeze_store::{Mode, StoreError};

    use super::Options;

    const LISTEN: Duration = Duration::from_secs(3);
    /// How many times a mistyped password may be typed again.
    const PASSWORD_TRIES: usize = 3;

    pub fn run(options: Options) -> Result<i32, String> {
        let path = options
            .config
            .clone()
            .unwrap_or_else(crate::cli::pair::default_config_path);
        let mut config: AppConfig = match breeze_store::load(&path) {
            Ok(c) => c,
            Err(StoreError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "there is no config.json at {path} yet. Run `breeze-core pair` first: it \
                     finds the units on the network and writes it, and then this can fill in \
                     their tokens."
                ));
            }
            Err(e) => {
                return Err(format!(
                    "cannot read {path}: {e}. It is readable by root and the breeze group, so \
                     run this with sudo (or as a member of that group)."
                ))
            }
        };

        // --- which units ---------------------------------------------------
        let targets = targets(&config, &options.units)?;
        if targets.is_empty() {
            println!(
                "Every unit in {path} already has its token and key, or needs none.\n\
                 To fetch one again anyway, name it: breeze-core fetch \"{}\"",
                config
                    .units
                    .first()
                    .map(|u| u.name.as_str())
                    .unwrap_or("Bedroom")
            );
            return Ok(0);
        }

        heading("What this does");
        say(
            "A V3 air conditioner will not talk to anything without a token and a key, and \
             only Midea hands them out -- these days only to the account the unit is paired \
             with in Midea's app. So this signs in to that account, once, asks for the \
             tokens, and checks each one on the unit itself before saving it.",
        );
        say(
            "Your password is used for that one sign-in and then forgotten. It is never \
             saved, logged or shown.",
        );

        // Only V3 units have tokens. Ask each one which it is, rather than fetching
        // for a V2 unit that needs nothing.
        heading("Checking the units");
        let mut wanted: Vec<usize> = Vec::new();
        for index in targets {
            let unit = &config.units[index];
            match probe(unit) {
                Probe::V3 => {
                    println!("  {:<20} {}  V3, needs a token and key", unit.name, unit.ip);
                    wanted.push(index);
                }
                Probe::Other(version) => println!(
                    "  {:<20} {}  {version}: needs no token, skipping it",
                    unit.name, unit.ip
                ),
                Probe::Silent => {
                    println!(
                        "  {:<20} {}  did not answer. Fetching anyway; its token can only be \
                         checked once it does.",
                        unit.name, unit.ip
                    );
                    wanted.push(index);
                }
                Probe::Moved(other) => println!(
                    "  {:<20} {}  a different unit answers there now (id {other}). Run \
                     `breeze-core pair` to update the addresses, then this again.",
                    unit.name, unit.ip
                ),
            }
        }
        if wanted.is_empty() {
            say("Nothing here needs a token.");
            return Ok(0);
        }

        // --- which app, which account --------------------------------------
        let choice = match &options.cloud {
            Some(name) => parse_cloud(name)?,
            None => ask_app()?,
        };
        let account = match &options.account {
            Some(a) => a.trim().to_string(),
            None => loop {
                let typed = ask("\nThe email (or phone number) you sign in to that app with: ")?
                    .ok_or("no account given")?;
                if !typed.is_empty() {
                    break typed;
                }
            },
        };

        heading("Which of Midea's clouds knows this account");
        say("Asking each of them by name only. No password is sent for this.");
        let known = who_knows(&account, choice);

        let cloud = match decide(choice, &known)? {
            Some(cloud) => cloud,
            None => {
                // Not echoed: this output is what people paste into a bug
                // report, and the address is theirs, not the report's.
                say(
                    "None of them has that account. Check how it is spelled -- it has to be \
                     exactly what you sign in to the app with -- or use the account of whoever \
                     set the unit up.",
                );
                return Ok(1);
            }
        };

        if cloud == Cloud::NetHomePlus {
            return nethome(&account, &options, &path, &mut config, &wanted, &known);
        }
        let session = match sign_in(cloud, &account, &options)? {
            Some(s) => s,
            None => return Ok(1),
        };
        fetch_all(cloud, &session, &path, &mut config, &wanted)
    }

    // --- choosing --------------------------------------------------------------

    /// What the person said about the app, before anything is asked of a cloud.
    #[derive(Clone, Copy, PartialEq)]
    enum Choice {
        App(Cloud),
        NotSure,
    }

    fn parse_cloud(name: &str) -> Result<Choice, String> {
        match name
            .to_ascii_lowercase()
            .replace(['-', ' ', '_'], "")
            .as_str()
        {
            "smarthome" | "msmarthome" => Ok(Choice::App(Cloud::SmartHome)),
            "meiju" => Ok(Choice::App(Cloud::Meiju)),
            "nethome" | "nethomeplus" => Ok(Choice::App(Cloud::NetHomePlus)),
            other => Err(format!(
                "unknown --cloud '{other}': smarthome, meiju or nethome"
            )),
        }
    }

    fn ask_app() -> Result<Choice, String> {
        heading("Which app is the unit paired with");
        say("The app you set the air conditioner up in, on your phone:");
        println!("  1  MSmartHome     Midea's current app, outside China");
        println!("  2  NetHome Plus   the older app many units came with");
        println!("  3  Meiju          美的美居, in mainland China");
        println!("  4  not sure       ask each of them about your account");
        loop {
            match ask("\nChoose 1-4: ")?.as_deref().map(str::trim) {
                Some("1") => return Ok(Choice::App(Cloud::SmartHome)),
                Some("2") => return Ok(Choice::App(Cloud::NetHomePlus)),
                Some("3") => return Ok(Choice::App(Cloud::Meiju)),
                Some("4") => return Ok(Choice::NotSure),
                Some(_) => println!("  1, 2, 3 or 4, please."),
                None => return Err("no answer".into()),
            }
        }
    }

    /// Whether each cloud has the account, printed as it goes. A cloud that
    /// cannot be reached is `None`, and said so, rather than taken as a no.
    fn who_knows(account: &str, choice: Choice) -> Vec<(Cloud, Option<bool>)> {
        // The chosen one first; then the others, because the most useful answer to
        // "MSmartHome doesn't know me" is which app does.
        let mut order = vec![Cloud::SmartHome, Cloud::NetHomePlus, Cloud::Meiju];
        if let Choice::App(c) = choice {
            order.retain(|o| *o != c);
            order.insert(0, c);
        }
        order
            .into_iter()
            .map(|cloud| {
                let answer = match cloud.account_known(account) {
                    Ok(known) => {
                        println!(
                            "  {:<16} {}",
                            cloud.short_name(),
                            if known {
                                "has this account"
                            } else {
                                "does not know it"
                            }
                        );
                        Some(known)
                    }
                    Err(e) => {
                        println!("  {:<16} could not be asked: {e}", cloud.short_name());
                        None
                    }
                };
                (cloud, answer)
            })
            .collect()
    }

    fn knows(known: &[(Cloud, Option<bool>)], cloud: Cloud) -> bool {
        known.iter().any(|(c, k)| *c == cloud && *k == Some(true))
    }

    /// Which cloud to sign in to, given what was chosen and who knows the
    /// account. `None` when nobody does.
    fn decide(choice: Choice, known: &[(Cloud, Option<bool>)]) -> Result<Option<Cloud>, String> {
        // Where a token can actually come from, in the order worth trying.
        let issuing = [Cloud::SmartHome, Cloud::Meiju]
            .into_iter()
            .find(|c| knows(known, *c));

        match choice {
            Choice::App(Cloud::NetHomePlus) => {
                if let Some(other) = issuing {
                    // The good case: the same account exists where tokens still are.
                    say(&format!(
                        "\n{} has this account too, and unlike NetHome Plus it still hands \
                         out tokens -- if the unit is paired there.",
                        other.app_name()
                    ));
                    if yes_no(&format!("Use {} instead?", other.app_name()), true)? {
                        return Ok(Some(other));
                    }
                }
                Ok(knows(known, Cloud::NetHomePlus).then_some(Cloud::NetHomePlus))
            }
            Choice::App(cloud) if knows(known, cloud) => Ok(Some(cloud)),
            Choice::App(cloud) => {
                say(&format!(
                    "\n{} does not know this account. It said so before any password was \
                     sent, so this is not about the password.",
                    cloud.app_name()
                ));
                if let Some(other) = issuing {
                    say(&format!("{} does, so I'll use that.", other.app_name()));
                    return Ok(Some(other));
                }
                if knows(known, Cloud::NetHomePlus) {
                    say(
                        "NetHome Plus does: this is a NetHome Plus account. Accounts made there \
                         do not exist in MSmartHome -- Midea keeps them on separate systems -- \
                         so here is what that means for the token.",
                    );
                    return Ok(Some(Cloud::NetHomePlus));
                }
                Ok(None)
            }
            Choice::NotSure => {
                Ok(issuing
                    .or_else(|| knows(known, Cloud::NetHomePlus).then_some(Cloud::NetHomePlus)))
            }
        }
    }

    // --- signing in ------------------------------------------------------------

    fn password(account: &str, cloud: Cloud, options: &Options) -> Result<String, String> {
        if options.password_stdin {
            let mut line = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut line)
                .map_err(|e| format!("cannot read the password from standard input: {e}"))?;
            return Ok(line.trim_end_matches(['\r', '\n']).to_string());
        }
        rpassword::prompt_password(format!(
            "\nPassword for {account} on {} (it will not show as you type): ",
            cloud.app_name()
        ))
        .map_err(|e| format!("cannot read the password: {e}"))
    }

    /// Sign in, letting a mistyped password be typed again.
    fn sign_in(cloud: Cloud, account: &str, options: &Options) -> Result<Option<Session>, String> {
        let tries = if options.password_stdin {
            1
        } else {
            PASSWORD_TRIES
        };
        for attempt in 1..=tries {
            let credentials = Credentials::new(account, password(account, cloud, options)?, "");
            match cloud.login(&credentials) {
                Ok(session) => {
                    println!("  Signed in to {}.", cloud.app_name());
                    return Ok(Some(session));
                }
                Err(CloudError::Api {
                    code: 3101 | 3102, ..
                }) if attempt < tries => {
                    println!(
                        "  That password was not accepted. ({} more {})",
                        tries - attempt,
                        if tries - attempt == 1 { "try" } else { "tries" }
                    );
                }
                Err(e) => {
                    println!("  Could not sign in: {e}");
                    say(e.advice());
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    // --- fetching --------------------------------------------------------------

    fn fetch_all(
        cloud: Cloud,
        session: &Session,
        path: &str,
        config: &mut AppConfig,
        wanted: &[usize],
    ) -> Result<i32, String> {
        // The account's own list, so a unit that is not on it can be explained
        // rather than reported as a bare refusal.
        let listed = match session.appliances() {
            Ok(Some(list)) => {
                heading(&format!("Units on this {} account", cloud.app_name()));
                if list.is_empty() {
                    say("None at all. The unit has to be added in the app first.");
                }
                for a in &list {
                    let known_as = config
                        .units
                        .iter()
                        .find(|u| u64::try_from(u.id).ok() == Some(a.id))
                        .map(|u| format!("  (\"{}\" here)", u.name))
                        .unwrap_or_default();
                    println!(
                        "  {:<20} {}{known_as}",
                        a.name,
                        if a.online { "online" } else { "offline" }
                    );
                }
                Some(list)
            }
            Ok(None) => None,
            Err(e) => {
                println!("  (could not list the account's units: {e})");
                None
            }
        };

        heading("Fetching and checking");
        let mut got: Vec<(usize, Token)> = Vec::new();
        let mut missing = 0usize;
        for &index in wanted {
            let unit = config.units[index].clone();
            let Ok(id) = u64::try_from(unit.id) else {
                continue;
            };
            if let Some(list) = &listed {
                if !list.iter().any(|a| a.id == id) {
                    missing += 1;
                    println!("  {}: not on this account.", unit.name);
                    say(&format!(
                        "    {} only gives a unit's token to the account it is paired with, and \
                         this unit is not one of the units listed above. If someone else set it \
                         up, fetch with their account; if it is still in another app, it has to \
                         be added in {} first.",
                        cloud.app_name(),
                        cloud.app_name()
                    ));
                    continue;
                }
            }
            match session.token(id) {
                Ok(token) => {
                    if check(&unit, &token)? {
                        got.push((index, token));
                    } else {
                        // Refused by the unit and not kept: still without one.
                        missing += 1;
                    }
                }
                Err(e) => {
                    missing += 1;
                    println!("  {}: no token -- {e}", unit.name);
                    say(&format!("    {}", e.advice()));
                }
            }
        }

        if got.is_empty() {
            say("\nNothing was saved; config.json is as it was.");
            return Ok(1);
        }
        save(path, config, &got)?;
        Ok(if missing == 0 { 0 } else { 1 })
    }

    /// Try the token on the unit itself. True to keep it.
    fn check(unit: &UnitConfig, token: &Token) -> Result<bool, String> {
        println!(
            "  {}: token received. Trying it on the unit itself...",
            unit.name
        );
        let mut candidate = unit.clone();
        candidate.token = Some(token.token.clone());
        candidate.key = Some(token.key.clone());
        let Some(device_unit) = breeze_http::state::to_device_unit(&candidate) else {
            println!(
                "  {}: the token Midea sent is not the right shape; not saving it.",
                unit.name
            );
            return Ok(false);
        };
        match breeze_device::Device::new(device_unit).refresh() {
            Ok(_) => {
                println!(
                    "  OK   {} accepted it: a V3 handshake and a state read, nothing changed on \
                     the unit.",
                    unit.name
                );
                Ok(true)
            }
            Err(e) => {
                println!("  !!   {} did not accept it: {e}", unit.name);
                say(
                    "    That usually means the unit is off or cannot be reached from here; it \
                     can also mean the unit was paired again since, and has a newer token.",
                );
                yes_no(
                    "    Save it anyway? It is the token Midea holds for this unit.",
                    false,
                )
            }
        }
    }

    fn save(path: &str, config: &mut AppConfig, got: &[(usize, Token)]) -> Result<(), String> {
        let backup = format!("{path}.before-fetch");
        breeze_store::save(&backup, config, Mode::AdminReadable)
            .map_err(|e| format!("cannot write the backup {backup}: {e}"))?;
        for (index, token) in got {
            config.units[*index].token = Some(token.token.clone());
            config.units[*index].key = Some(token.key.clone());
        }
        breeze_store::save(path, config, Mode::AdminReadable)
            .map_err(|e| format!("cannot write {path}: {e}"))?;

        heading("Saved");
        let names: Vec<&str> = got
            .iter()
            .map(|(i, _)| config.units[*i].name.as_str())
            .collect();
        say(&format!(
            "{} now in {path}. The version before this is in {backup}.",
            names.join(", ")
        ));
        say("The server reads config.json when it starts, so restart it to use them:");
        println!("    {}", restart_hint());
        say(
            "\nKeep a copy of config.json somewhere off this machine too. A unit never forgets \
             its token, but Midea may stop handing them out altogether, and then that copy is \
             the only one there is.",
        );
        Ok(())
    }

    // --- NetHome Plus ------------------------------------------------------------

    fn nethome(
        account: &str,
        options: &Options,
        path: &str,
        config: &mut AppConfig,
        wanted: &[usize],
        known: &[(Cloud, Option<bool>)],
    ) -> Result<i32, String> {
        heading("NetHome Plus");
        say(
            "NetHome Plus still lets you sign in, but since August 2026 it has refused to hand \
             out tokens -- for every unit, its owners' included -- and nobody has found a way \
             round that. I'll try once, in case it has changed.",
        );
        if let Some(session) = sign_in(Cloud::NetHomePlus, account, options)? {
            let first = &config.units[wanted[0]];
            match u64::try_from(first.id).ok().map(|id| session.token(id)) {
                Some(Ok(_)) => {
                    say("It answered with a token -- NetHome Plus is issuing them again.");
                    return fetch_all(Cloud::NetHomePlus, &session, path, config, wanted);
                }
                Some(Err(e)) => println!("  As expected, it refused: {e}"),
                None => {}
            }
        }

        heading("What works instead");
        say(
            "MSmartHome, Midea's current app, still hands out tokens -- to the account the unit \
             is paired with. A NetHome Plus login is not an MSmartHome account (Midea keeps \
             them on separate systems), so the unit has to move:",
        );
        let smarthome_known = knows(known, Cloud::SmartHome);
        println!("  1. Install MSmartHome on your phone.");
        if smarthome_known {
            println!("  2. Sign in with the same email or phone number -- MSmartHome already");
            println!("     knows it.");
        } else {
            println!("  2. Create an account in it. The same email is fine; it is a new account.");
        }
        println!("  3. Add the air conditioner in MSmartHome. The app walks you through putting");
        println!("     the unit into pairing mode and onto your Wi-Fi again.");
        println!("  4. Run `breeze-core fetch` again and choose MSmartHome.");

        say(
            "\nOnly do this for units that need a token now. Pairing a unit again gives it a new \
             token, so the one Breeze holds for it will most likely stop working.",
        );
        let fine: Vec<&str> = config
            .units
            .iter()
            .filter(|u| u.token.is_some() && u.key.is_some())
            .map(|u| u.name.as_str())
            .collect();
        if !fine.is_empty() {
            say(&format!(
                "These already have their token and key, so leave them as they are: {}.",
                fine.join(", ")
            ));
        }
        say(
            "\nIf you have a unit's token and key from somewhere else -- an old Home Assistant \
             setup, or msmart-ng's output -- you can type them in instead:",
        );
        for &index in wanted {
            println!("    breeze-core pair --ip {}", config.units[index].ip);
        }
        Ok(1)
    }

    // --- the units ---------------------------------------------------------------

    /// Every unit with no token, or the ones named.
    fn targets(config: &AppConfig, asked: &[String]) -> Result<Vec<usize>, String> {
        if asked.is_empty() {
            return Ok(config
                .units
                .iter()
                .enumerate()
                .filter(|(_, u)| u.token.is_none() || u.key.is_none())
                .map(|(i, _)| i)
                .collect());
        }
        let mut out = Vec::new();
        for name in asked {
            let found = config.units.iter().position(|u| {
                u.name.eq_ignore_ascii_case(name) || u.ip == *name || u.id.to_string() == *name
            });
            match found {
                Some(i) if !out.contains(&i) => out.push(i),
                Some(_) => {}
                None => {
                    let names: Vec<&str> = config.units.iter().map(|u| u.name.as_str()).collect();
                    return Err(format!(
                        "no unit called or at '{name}' in config.json. The units it has: {}",
                        if names.is_empty() {
                            "none".to_string()
                        } else {
                            names.join(", ")
                        }
                    ));
                }
            }
        }
        Ok(out)
    }

    enum Probe {
        V3,
        Other(String),
        Silent,
        Moved(u64),
    }

    /// What answers at the unit's address: the same unicast probe `pair --ip`
    /// sends, so the reply can only be from that address.
    fn probe(unit: &UnitConfig) -> Probe {
        let Ok(report) = breeze_device::scan_subnet(&format!("{}/32", unit.ip.trim()), LISTEN)
        else {
            return Probe::Silent;
        };
        match report.found.first() {
            None => Probe::Silent,
            Some(d) if i64::try_from(d.id).ok() != Some(unit.id) => Probe::Moved(d.id),
            Some(d) if d.version == Version::V3 => Probe::V3,
            Some(d) => Probe::Other(format!("{:?}", d.version)),
        }
    }

    /// The restart command for whatever is managing the service here.
    fn restart_hint() -> &'static str {
        use std::path::Path;
        if cfg!(windows) {
            "Restart-Service BreezeCore        (in an administrator PowerShell)"
        } else if cfg!(target_os = "android") {
            "sv restart breeze-core"
        } else if cfg!(target_os = "openbsd") {
            "doas rcctl restart breeze_core"
        } else if cfg!(any(target_os = "freebsd", target_os = "netbsd")) {
            "sudo service breeze_core restart"
        } else if Path::new("/run/systemd/system").is_dir() {
            "sudo systemctl restart breeze-core"
        } else if Path::new("/run/openrc").exists() {
            "sudo rc-service breeze-core restart"
        } else if Path::new("/sbin/procd").exists() {
            "/etc/init.d/breeze-core restart"
        } else {
            "restart the breeze-core service with your init system"
        }
    }

    // --- talking ---------------------------------------------------------------

    fn heading(title: &str) {
        println!("\n== {title} ==");
    }

    /// A paragraph, wrapped to the terminal's usual width. Leading newlines
    /// become blank lines; leading spaces, the indent (two at least).
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

    /// A prompt, `None` at end of input, so a script that runs out of answers
    /// stops instead of looping.
    fn ask(message: &str) -> Result<Option<String>, String> {
        print!("{message}");
        std::io::stdout()
            .flush()
            .map_err(|e| format!("cannot write to the terminal: {e}"))?;
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

        fn unit(name: &str, ip: &str, id: i64, token: bool) -> UnitConfig {
            UnitConfig {
                name: name.into(),
                ip: ip.into(),
                port: 6444,
                id,
                token: token.then(|| "a".repeat(128)),
                key: token.then(|| "b".repeat(64)),
            }
        }

        fn config() -> AppConfig {
            AppConfig {
                api_key: Some("k".into()),
                units: vec![
                    unit("Bedroom", "192.0.2.10", 1, false),
                    unit("Living room", "192.0.2.11", 2, true),
                    unit("Kitchen", "192.0.2.12", 3, false),
                ],
            }
        }

        #[test]
        fn by_default_only_the_units_without_credentials_are_fetched() {
            assert_eq!(targets(&config(), &[]).unwrap(), vec![0, 2]);
        }

        #[test]
        fn a_unit_can_be_named_by_name_address_or_id_once() {
            let asked = ["living ROOM".to_string(), "192.0.2.12".into(), "2".into()];
            assert_eq!(targets(&config(), &asked).unwrap(), vec![1, 2]);
            let err = targets(&config(), &["Garage".to_string()]).unwrap_err();
            assert!(err.contains("Bedroom, Living room, Kitchen"), "{err}");
        }

        #[test]
        fn the_cloud_flag_takes_the_names_people_use() {
            for name in ["smarthome", "MSmartHome", "msmart-home"] {
                assert!(matches!(
                    parse_cloud(name),
                    Ok(Choice::App(Cloud::SmartHome))
                ));
            }
            assert!(matches!(
                parse_cloud("NetHome Plus"),
                Ok(Choice::App(Cloud::NetHomePlus))
            ));
            assert!(matches!(
                parse_cloud("meiju"),
                Ok(Choice::App(Cloud::Meiju))
            ));
            assert!(parse_cloud("alexa").is_err());
        }

        #[test]
        fn a_nethome_account_known_to_smarthome_too_is_sent_to_smarthome_when_unsure() {
            let known = [
                (Cloud::SmartHome, Some(true)),
                (Cloud::NetHomePlus, Some(true)),
                (Cloud::Meiju, Some(false)),
            ];
            assert_eq!(
                decide(Choice::NotSure, &known).unwrap(),
                Some(Cloud::SmartHome)
            );
        }

        #[test]
        fn an_account_only_nethome_knows_goes_to_the_nethome_help() {
            let known = [
                (Cloud::SmartHome, Some(false)),
                (Cloud::NetHomePlus, Some(true)),
                (Cloud::Meiju, Some(false)),
            ];
            assert_eq!(
                decide(Choice::NotSure, &known).unwrap(),
                Some(Cloud::NetHomePlus)
            );
            assert_eq!(
                decide(Choice::App(Cloud::SmartHome), &known).unwrap(),
                Some(Cloud::NetHomePlus),
                "chose MSmartHome, but it is a NetHome Plus account"
            );
        }

        #[test]
        fn nobody_knowing_the_account_is_none_and_unreachable_is_not_a_no() {
            let known = [
                (Cloud::SmartHome, Some(false)),
                (Cloud::NetHomePlus, None),
                (Cloud::Meiju, Some(false)),
            ];
            assert_eq!(decide(Choice::NotSure, &known).unwrap(), None);
            assert!(!knows(&known, Cloud::NetHomePlus));
        }
    }
}
