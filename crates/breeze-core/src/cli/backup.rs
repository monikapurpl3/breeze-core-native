//! `breeze-core backup` and `breeze-core restore`: the four store files,
//! encrypted with a passphrase.
//!
//! Run on the server's own host, like `pair`: they read and write the files
//! directly, never through the API, so no secret ever crosses the network and
//! a server that will not start can still be restored.
//!
//! The file is Breeze Core's own format:
//!
//! ```text
//! offset size
//!   0     8   magic         "BREEZEBK"
//!   8     1   format        1
//!   9     1   kdf           1 = Argon2id, version 0x13
//!  10     4   memory        KiB, little-endian
//!  14     4   passes        little-endian
//!  18     4   lanes         little-endian
//!  22    16   salt
//!  38    12   nonce
//!  50     -   AES-256-GCM ciphertext, then its 16-byte tag
//! ```
//!
//! The 50 header bytes are the cipher's associated data: none of them can be
//! changed -- the cost lowered, say -- without the tag failing. The key is
//! Argon2id of the passphrase and the salt, with the cost the header names, so
//! a later version can raise the cost and still open every older backup.
//!
//! Inside is JSON: who made it and when, and each store file's text exactly as
//! it was on disk (or null where there was none). Exactly, because the store
//! files are byte-for-byte what the Python reference wrote, and a restore that
//! re-serialised them would quietly break that.

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::PathBuf;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use breeze_store::{AppConfig, DevicesDoc, Mode, ProgramsDoc, TimersDoc};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize as _, Zeroizing};

const MAGIC: &[u8; 8] = b"BREEZEBK";
const FORMAT: u8 = 1;
const KDF_ARGON2ID: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = 8 + 1 + 1 + 4 + 4 + 4 + SALT_LEN + NONCE_LEN;

/// Argon2id's cost for every new backup: OWASP's recommended minimum, 19 MiB
/// and two passes on one lane. Enough to make guessing slow; little enough
/// that a router can open a backup made on a PC, which a 64 MiB setting would
/// not reliably allow.
pub const COST: Cost = Cost {
    memory_kib: 19 * 1024,
    passes: 2,
    lanes: 1,
};

/// The most a backup may ask for when it is opened. A backup is data, and one
/// claiming a terabyte of memory is refused rather than attempted.
const MAX_MEMORY_KIB: u32 = 1024 * 1024;
const MAX_PASSES: u32 = 64;
const MAX_LANES: u32 = 16;

/// The shortest passphrase a new backup takes. It is all that stands between
/// whoever has the file and every key in it, offline, at their leisure.
pub const MIN_PASSPHRASE: usize = 10;

/// The extension of a generated backup's name.
pub const EXTENSION: &str = "breeze-backup";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    pub memory_kib: u32,
    pub passes: u32,
    pub lanes: u32,
}

/// Why a backup could not be opened.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    NotABackup,
    /// Made by a newer Breeze Core, in a format this one does not know.
    UnknownFormat(u8),
    UnknownKdf(u8),
    /// Argon2 parameters out of range: zero, or more than [`MAX_MEMORY_KIB`]
    /// and friends allow.
    BadCost(Cost),
    Truncated,
    /// The tag did not verify. AES-GCM cannot say which of the two it was,
    /// and that is by design: a wrong passphrase and a changed file look
    /// identical, so neither tells an attacker anything.
    WrongPassphraseOrChanged,
}

impl core::fmt::Display for OpenError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotABackup => write!(f, "this is not a Breeze Core backup"),
            Self::UnknownFormat(v) => write!(
                f,
                "this backup is in format {v}, which a newer Breeze Core wrote; \
                 restore it with that version"
            ),
            Self::UnknownKdf(v) => write!(f, "this backup uses key derivation {v}, unknown here"),
            Self::BadCost(c) => write!(
                f,
                "this backup asks for Argon2 memory {} KiB, {} passes, {} lanes, \
                 which is refused",
                c.memory_kib, c.passes, c.lanes
            ),
            Self::Truncated => write!(f, "this backup is cut short"),
            Self::WrongPassphraseOrChanged => write!(
                f,
                "the passphrase is wrong, or the file has been changed since it was made"
            ),
        }
    }
}

/// The key for `passphrase` and `salt` at `cost`.
fn derive(passphrase: &[u8], salt: &[u8], cost: Cost) -> Result<Zeroizing<[u8; 32]>, String> {
    let params = argon2::Params::new(cost.memory_kib, cost.passes, cost.lanes, Some(32))
        .map_err(|e| format!("Argon2 parameters: {e}"))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase, salt, key.as_mut())
        .map_err(|e| format!("Argon2: {e}"))?;
    Ok(key)
}

/// Encrypt `plaintext` into a backup file's bytes.
pub fn seal(plaintext: &[u8], passphrase: &[u8], cost: Cost) -> Result<Vec<u8>, String> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut salt).map_err(|e| format!("no randomness for the salt: {e}"))?;
    getrandom::getrandom(&mut nonce).map_err(|e| format!("no randomness for the nonce: {e}"))?;
    seal_with(plaintext, passphrase, cost, &salt, &nonce)
}

fn seal_with(
    plaintext: &[u8],
    passphrase: &[u8],
    cost: Cost,
    salt: &[u8; SALT_LEN],
    nonce: &[u8; NONCE_LEN],
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(HEADER_LEN + plaintext.len() + TAG_LEN);
    out.extend_from_slice(MAGIC);
    out.push(FORMAT);
    out.push(KDF_ARGON2ID);
    out.extend_from_slice(&cost.memory_kib.to_le_bytes());
    out.extend_from_slice(&cost.passes.to_le_bytes());
    out.extend_from_slice(&cost.lanes.to_le_bytes());
    out.extend_from_slice(salt);
    out.extend_from_slice(nonce);
    debug_assert_eq!(out.len(), HEADER_LEN);

    let key = derive(passphrase, salt, cost)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| "bad key length")?;
    let sealed = cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad: &out,
            },
        )
        .map_err(|_| "encryption failed")?;
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Decrypt a backup file's bytes.
pub fn open(file: &[u8], passphrase: &[u8]) -> Result<Zeroizing<Vec<u8>>, OpenError> {
    if !file.starts_with(MAGIC) {
        return Err(OpenError::NotABackup);
    }
    if file.len() < HEADER_LEN + TAG_LEN {
        return Err(OpenError::Truncated);
    }
    let (header, body) = file.split_at(HEADER_LEN);
    if header[8] != FORMAT {
        return Err(OpenError::UnknownFormat(header[8]));
    }
    if header[9] != KDF_ARGON2ID {
        return Err(OpenError::UnknownKdf(header[9]));
    }
    let u32_at =
        |i: usize| u32::from_le_bytes([header[i], header[i + 1], header[i + 2], header[i + 3]]);
    let cost = Cost {
        memory_kib: u32_at(10),
        passes: u32_at(14),
        lanes: u32_at(18),
    };
    if cost.memory_kib > MAX_MEMORY_KIB
        || cost.passes == 0
        || cost.passes > MAX_PASSES
        || cost.lanes == 0
        || cost.lanes > MAX_LANES
    {
        return Err(OpenError::BadCost(cost));
    }
    let key = derive(passphrase, &header[22..38], cost).map_err(|_| OpenError::BadCost(cost))?;
    let cipher =
        Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| OpenError::WrongPassphraseOrChanged)?;
    cipher
        .decrypt(
            Nonce::from_slice(&header[38..50]),
            Payload {
                msg: body,
                aad: header,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| OpenError::WrongPassphraseOrChanged)
}

// ------------------------------------------------------------------ contents

/// What is inside, once decrypted.
#[derive(Debug, Serialize, Deserialize)]
struct Contents {
    breeze_core_backup: u32,
    created_at: String,
    created_by: String,
    hostname: Option<String>,
    files: Files,
}

/// Each store file's text, exactly as it was; `None` where there was no file.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Files {
    config: Option<String>,
    devices: Option<String>,
    programs: Option<String>,
    timers: Option<String>,
}

impl Files {
    fn get(&self, role: Role) -> Option<&String> {
        match role {
            Role::Config => self.config.as_ref(),
            Role::Devices => self.devices.as_ref(),
            Role::Programs => self.programs.as_ref(),
            Role::Timers => self.timers.as_ref(),
        }
    }

    fn set(&mut self, role: Role, text: String) {
        let slot = match role {
            Role::Config => &mut self.config,
            Role::Devices => &mut self.devices,
            Role::Programs => &mut self.programs,
            Role::Timers => &mut self.timers,
        };
        *slot = Some(text);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Config,
    Devices,
    Programs,
    Timers,
}

impl Role {
    const ALL: [Role; 4] = [Role::Config, Role::Devices, Role::Programs, Role::Timers];

    fn name(self) -> &'static str {
        match self {
            Role::Config => "config.json",
            Role::Devices => "devices.json",
            Role::Programs => "programs.json",
            Role::Timers => "timers.json",
        }
    }

    /// The same modes the server writes them with.
    fn mode(self) -> Mode {
        match self {
            Role::Config => Mode::AdminReadable,
            _ => Mode::Private,
        }
    }

    fn path(self, settings: &breeze_http::Settings) -> PathBuf {
        match self {
            Role::Config => settings.config_path.clone(),
            Role::Devices => settings.devices_path.clone(),
            Role::Programs => settings.programs_path.clone(),
            Role::Timers => settings.timers_path.clone(),
        }
    }

    /// Parse `text` as this file, and say in a few words what it holds.
    ///
    /// Run on everything before anything is written, and on everything before
    /// it is backed up: a backup of a broken file would restore a server that
    /// cannot start.
    fn check(self, text: &str) -> Result<String, String> {
        if text.trim().is_empty() {
            // How the server treats a half-written file: as absent.
            return Ok("empty".into());
        }
        let count =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let parsed = match self {
            Role::Config => serde_json::from_str::<AppConfig>(text).map(|c| {
                let key = if c.api_key.is_some() {
                    ", the API key"
                } else {
                    ""
                };
                count(c.units.len(), "unit", "units") + key
            }),
            Role::Devices => serde_json::from_str::<DevicesDoc>(text)
                .map(|d| count(d.devices.len(), "paired device", "paired devices")),
            Role::Programs => serde_json::from_str::<ProgramsDoc>(text)
                .map(|p| count(p.programs.len(), "program", "programs")),
            Role::Timers => serde_json::from_str::<TimersDoc>(text)
                .map(|t| count(t.timers.len(), "timer", "timers")),
        };
        parsed.map_err(|e| format!("{} is not valid: {e}", self.name()))
    }
}

// ------------------------------------------------------------------ commands

#[derive(Debug, Default)]
pub struct BackupOptions {
    /// Where to write it. Default: a generated name here, or in the home folder.
    pub out: Option<String>,
    pub passphrase_stdin: bool,
}

#[derive(Debug, Default)]
pub struct RestoreOptions {
    pub file: String,
    pub passphrase_stdin: bool,
    /// Do not ask before replacing the files.
    pub yes: bool,
    /// Restore even while a server is answering.
    pub force: bool,
}

pub fn backup(options: BackupOptions) -> Result<(), String> {
    let settings = breeze_http::Settings::from_env();
    let mut files = Files::default();
    let mut held = Vec::new();
    for role in Role::ALL {
        let path = role.path(&settings);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let summary = role
                    .check(&text)
                    .map_err(|e| format!("{e}\n  {} was not backed up.", path.display()))?;
                held.push(format!("{:<14} {summary}", role.name()));
                files.set(role, text);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                held.push(format!("{:<14} none yet", role.name()));
            }
            Err(e) => return Err(unreadable(&path, &e)),
        }
    }
    if files.config.is_none() {
        return Err(format!(
            "there is nothing to back up: no config.json at {}\n  \
             (set AC_CONFIG_DIR if the server keeps its files somewhere else)",
            settings.config_path.display()
        ));
    }

    let passphrase = passphrase(options.passphrase_stdin, true)?;
    if passphrase.chars().count() < MIN_PASSPHRASE {
        return Err(format!(
            "the passphrase needs at least {MIN_PASSPHRASE} characters: it is all that \
             protects every key in the backup. A few unrelated words make a good one."
        ));
    }

    let hostname = breeze_http::system::hostname();
    let now = breeze_store::now_local();
    let contents = Contents {
        breeze_core_backup: 1,
        created_at: now.format("%Y-%m-%dT%H:%M:%S").to_string(),
        created_by: format!("breeze-core {}", env!("CARGO_PKG_VERSION")),
        hostname: hostname.clone(),
        files,
    };
    let plain = Zeroizing::new(serde_json::to_vec(&contents).map_err(|e| e.to_string())?);
    let sealed = seal(&plain, passphrase.as_bytes(), COST)?;

    let saved = match options.out {
        // Named by the person: written there or not at all.
        Some(out) => {
            let out = PathBuf::from(out);
            super::files::write_private(&out, &sealed)
                .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
            out
        }
        None => {
            let host = super::files::tidy(hostname.as_deref().unwrap_or("server"));
            let name = format!(
                "breeze-core-backup-{host}-{}.{EXTENSION}",
                now.format("%Y%m%d-%H%M%S")
            );
            super::files::save_here_or_home(&name, &sealed)?
        }
    };

    println!("Backed up to {} ({} bytes):", saved.display(), sealed.len());
    for line in held {
        println!("  {line}");
    }
    println!(
        "\nIt holds every secret this server has: the API key, each unit's token and key,\n\
         every paired device. The passphrase is all that protects it, and it cannot be\n\
         recovered -- without it, the backup cannot be opened by anyone, you included.\n\
         Restore it with:  breeze-core restore {}",
        saved.display()
    );
    Ok(())
}

pub fn restore(options: RestoreOptions) -> Result<(), String> {
    let bytes = std::fs::read(&options.file).map_err(|e| format!("{}: {e}", options.file))?;
    // Before the passphrase, so a wrong file is not mistaken for a wrong one.
    if !bytes.starts_with(MAGIC) {
        return Err(format!("{} is not a Breeze Core backup", options.file));
    }
    let passphrase = passphrase(options.passphrase_stdin, false)?;
    let plain = open(&bytes, passphrase.as_bytes()).map_err(|e| e.to_string())?;
    let contents: Contents = serde_json::from_slice(&plain)
        .map_err(|e| format!("the backup opened, but what is inside is not valid: {e}"))?;
    if contents.breeze_core_backup != 1 {
        return Err(format!(
            "this backup's contents are version {}, which a newer Breeze Core wrote",
            contents.breeze_core_backup
        ));
    }

    // Every file checked before any is written: half a restore is worse than
    // none.
    let settings = breeze_http::Settings::from_env();
    let mut plan = Vec::new();
    println!(
        "Backup of {} made {} by {}:",
        contents.hostname.as_deref().unwrap_or("a server"),
        contents.created_at.replace('T', " "),
        contents.created_by
    );
    for role in Role::ALL {
        let path = role.path(&settings);
        match contents.files.get(role) {
            Some(text) => {
                let summary = role.check(text)?;
                println!("  {:<14} {summary:<28} -> {}", role.name(), path.display());
                plan.push((role, path, text));
            }
            None => println!("  {:<14} not in the backup; left as it is", role.name()),
        }
    }

    if let Some(at) = server_answering(&settings) {
        if !options.force {
            return Err(format!(
                "Breeze Core is running at {at}. Stop it first -- it keeps these files in\n  \
                 memory and would carry on with the old ones, writing them back over the\n  \
                 restored ones at its next change. Or pass --force and restart it after."
            ));
        }
        println!("\nBreeze Core is running at {at}; restoring anyway (--force).");
    }

    if !options.yes {
        confirm("\nReplace the files above with the backup's?")?;
    }
    for (role, path, text) in plan {
        breeze_store::save_text(&path, text, role.mode()).map_err(|e| e.to_string())?;
        println!("  restored {}", path.display());
    }
    println!("\nDone. Start Breeze Core again: it reads these files only when it starts.");
    Ok(())
}

/// A failure to read a store file, with the likeliest fix said out loud.
fn unreadable(path: &std::path::Path, e: &std::io::Error) -> String {
    let mut message = format!("{}: {e}", path.display());
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        message.push_str(
            "\n  note: the store files belong to the service account (breeze). \
             Run this with sudo, or as that user.",
        );
    }
    message
}

/// The passphrase: typed twice for a new backup, once to open one.
fn passphrase(from_stdin: bool, twice: bool) -> Result<Zeroizing<String>, String> {
    if from_stdin {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| format!("cannot read the passphrase from standard input: {e}"))?;
        let passphrase = Zeroizing::new(line.trim_end_matches(['\r', '\n']).to_string());
        line.zeroize();
        return Ok(passphrase);
    }
    let ask = |prompt: &str| {
        rpassword::prompt_password(prompt).map(Zeroizing::new).map_err(|e| {
            format!("cannot read the passphrase: {e}\n  From a script, pipe it in with --passphrase-stdin.")
        })
    };
    let first = ask("Passphrase (it will not show as you type): ")?;
    if twice {
        let again = ask("The same again: ")?;
        if *first != *again {
            return Err("the two passphrases differ; nothing was written".into());
        }
    }
    Ok(first)
}

/// Ask a yes/no question, where somebody is there to answer it.
fn confirm(question: &str) -> Result<(), String> {
    if !std::io::stdin().is_terminal() {
        return Err("nothing was restored: pass --yes to restore without being asked".into());
    }
    print!("{question} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("cannot read from the terminal: {e}"))?;
    if matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err("nothing was restored".into())
    }
}

/// Where a Breeze Core is answering on this machine, if one is.
///
/// The service's own address comes from its env file when there is one --
/// a shell running `sudo breeze-core restore` has not read it -- and then from
/// this process's environment, which is what the server would use too.
fn server_answering(settings: &breeze_http::Settings) -> Option<String> {
    let env_file = std::path::Path::new(&breeze_store::config_dir()).join("breeze-core.env");
    let from_file = |key: &str| {
        std::fs::read_to_string(&env_file)
            .ok()?
            .lines()
            .find_map(|line| {
                let (k, v) = line.trim().split_once('=')?;
                (k.trim() == key).then(|| v.trim().trim_matches(['"', '\'']).to_string())
            })
    };
    let (bind_host, bind_port) = settings.bind.rsplit_once(':')?;
    let host = from_file("BREEZE_HOST").unwrap_or_else(|| bind_host.to_string());
    let port = from_file("BREEZE_PORT").unwrap_or_else(|| bind_port.to_string());
    // Listening everywhere means listening here too.
    let host = match host.as_str() {
        "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1".to_string(),
        h if h.contains(':') && !h.starts_with('[') => format!("[{h}]"),
        h => h.to_string(),
    };
    let at = format!("{host}:{port}");
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(2))
        .build();
    let health: serde_json::Value = agent
        .get(&format!("http://{at}/api/health"))
        .call()
        .ok()?
        .into_json()
        .ok()?;
    (health["status"] == "ok").then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap, so the tests run in milliseconds. Real backups use [`COST`].
    const FAST: Cost = Cost {
        memory_kib: 64,
        passes: 1,
        lanes: 1,
    };

    #[test]
    fn what_is_sealed_opens_with_the_same_passphrase() {
        let sealed = seal(b"the store files", b"correct horse battery", FAST).unwrap();
        assert!(sealed.starts_with(b"BREEZEBK"));
        assert_eq!(sealed.len(), HEADER_LEN + 15 + TAG_LEN);
        let opened = open(&sealed, b"correct horse battery").unwrap();
        assert_eq!(opened.as_slice(), b"the store files");
    }

    #[test]
    fn two_backups_of_the_same_files_share_nothing() {
        // Fresh salt and nonce each time: nobody can tell two backups hold the
        // same thing, and no nonce is ever reused under one key.
        let a = seal(b"same", b"same passphrase", FAST).unwrap();
        let b = seal(b"same", b"same passphrase", FAST).unwrap();
        assert_ne!(a[22..50], b[22..50]);
        assert_ne!(a[HEADER_LEN..], b[HEADER_LEN..]);
    }

    #[test]
    fn a_wrong_passphrase_opens_nothing() {
        let sealed = seal(b"secret", b"correct horse battery", FAST).unwrap();
        assert_eq!(
            open(&sealed, b"correct horse batterY").unwrap_err(),
            OpenError::WrongPassphraseOrChanged
        );
    }

    #[test]
    fn every_byte_is_covered_header_included() {
        // Flip one bit anywhere past the magic and the format/kdf bytes (which
        // are rejected by name instead) -- cost, salt, nonce, ciphertext, tag --
        // and the file must not open. The cost bytes matter most: lowering
        // them unnoticed would be a downgrade.
        let sealed = seal(b"secret store files", b"correct horse battery", FAST).unwrap();
        for i in 10..sealed.len() {
            let mut changed = sealed.clone();
            changed[i] ^= 0x01;
            let result = open(&changed, b"correct horse battery");
            assert!(
                matches!(
                    result,
                    Err(OpenError::WrongPassphraseOrChanged) | Err(OpenError::BadCost(_))
                ),
                "byte {i} changed and the file still opened: {result:?}"
            );
        }
    }

    #[test]
    fn the_header_is_read_before_any_work_is_done() {
        assert_eq!(
            open(b"PK\x03\x04 a zip", b"x").unwrap_err(),
            OpenError::NotABackup
        );
        assert_eq!(
            open(b"BREEZEBK\x01\x01", b"x").unwrap_err(),
            OpenError::Truncated
        );
        let mut sealed = seal(b"x", b"correct horse battery", FAST).unwrap();
        sealed[8] = 2;
        assert_eq!(
            open(&sealed, b"x").unwrap_err(),
            OpenError::UnknownFormat(2)
        );
        sealed[8] = 1;
        sealed[9] = 7;
        assert_eq!(open(&sealed, b"x").unwrap_err(), OpenError::UnknownKdf(7));
    }

    #[test]
    fn a_backup_asking_for_absurd_memory_is_refused_not_attempted() {
        let mut sealed = seal(b"x", b"correct horse battery", FAST).unwrap();
        sealed[10..14].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(open(&sealed, b"x"), Err(OpenError::BadCost(_))));
    }

    #[test]
    fn the_real_cost_is_what_the_header_says() {
        let sealed = seal(b"x", b"correct horse battery", COST).unwrap();
        assert_eq!(&sealed[10..14], &(19 * 1024u32).to_le_bytes());
        assert_eq!(&sealed[14..18], &2u32.to_le_bytes());
        assert_eq!(&sealed[18..22], &1u32.to_le_bytes());
        assert_eq!(
            open(&sealed, b"correct horse battery").unwrap().as_slice(),
            b"x"
        );
    }

    /// The format, pinned by a known answer: a fixed salt and nonce must give
    /// these exact bytes forever, or every backup already made stops opening.
    #[test]
    fn the_format_does_not_drift() {
        let sealed = seal_with(b"{}", b"correct horse battery", FAST, &[7; 16], &[9; 12]).unwrap();
        let hex: String = sealed.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            &hex[..100],
            concat!(
                "425245455a45424b",
                "01",
                "01",
                "40000000",
                "01000000",
                "01000000",
                "07070707070707070707070707070707",
                "090909090909090909090909"
            )
        );
        assert_eq!(hex.len(), (HEADER_LEN + 2 + TAG_LEN) * 2);
        // The ciphertext and tag. Not this code's own output written down: the
        // same vector through argon2-cffi (the reference Argon2 C code) and
        // pyca/cryptography (OpenSSL's AES-GCM) gave these bytes first, on
        // 2026-11-15. Any change here is a format break, not a test to update.
        assert_eq!(&hex[100..], KNOWN_ANSWER_BODY);
    }

    const KNOWN_ANSWER_BODY: &str = "819d6e0ad30c871a96fb9452b85bd2a4b43c";

    #[test]
    fn store_files_are_checked_and_summarised() {
        assert_eq!(
            Role::Config
                .check(r#"{"api_key": "k", "units": []}"#)
                .unwrap(),
            "0 units, the API key"
        );
        assert_eq!(Role::Timers.check("  ").unwrap(), "empty");
        assert!(Role::Devices.check("{not json").is_err());
        assert_eq!(
            Role::Programs.check(r#"{"programs": []}"#).unwrap(),
            "0 programs"
        );
    }

    #[test]
    fn contents_keep_each_file_byte_for_byte() {
        // No trailing newline, two-space indent, a float with all its digits:
        // whatever the file was, the restore must write back exactly that.
        let original = "{\n  \"devices\": [],\n  \"x\": 1754000000.1234567\n}";
        let mut files = Files::default();
        files.set(Role::Devices, original.to_string());
        let contents = Contents {
            breeze_core_backup: 1,
            created_at: "2026-11-13T16:00:00".into(),
            created_by: "breeze-core test".into(),
            hostname: None,
            files,
        };
        let text = serde_json::to_vec(&contents).unwrap();
        let back: Contents = serde_json::from_slice(&text).unwrap();
        assert_eq!(back.files.devices.as_deref(), Some(original));
        assert!(back.files.config.is_none());
    }
}
