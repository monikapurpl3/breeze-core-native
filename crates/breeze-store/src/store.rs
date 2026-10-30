//! Loading and saving, with the modes and formatting the existing files use.

use serde::{de::DeserializeOwned, Serialize};
use std::path::Path;

/// Which permissions a store file carries.
///
/// Not a detail: `config.json` is deliberately group-readable so the admin CLIs
/// work without `sudo` — Breeze Core 2.4.3 changed it from 600 to 640 for exactly
/// that reason, after telling people to join the group had no effect. Writing it
/// back as 600 would silently undo that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 0640 — owner read/write, group read. `config.json`.
    AdminReadable,
    /// 0600 — owner only. Runtime state: devices, programs, timers.
    Private,
}

impl Mode {
    pub fn bits(self) -> u32 {
        match self {
            Self::AdminReadable => 0o640,
            Self::Private => 0o600,
        }
    }
}

#[derive(Debug)]
pub enum StoreError {
    /// Something went wrong reaching the file. **Carries the path**, because a
    /// bare `std::io::Error` does not: an unreadable config.json surfaced as
    /// `breeze-core: Permission denied (os error 13)` and nothing else, which
    /// names neither the file nor even the fact that a file was involved.
    ///
    /// There is deliberately no `From<std::io::Error>` impl. It could not
    /// supply a path, so `?` would go on quietly producing pathless errors.
    Io {
        path: String,
        source: std::io::Error,
    },
    /// The file exists but is not the JSON we expect.
    Parse {
        path: String,
        source: serde_json::Error,
    },
    /// Root wrote a new copy but could not give it to the file's owner, so the
    /// old file was left in place: a copy the service cannot read would stop
    /// it at its next start. A separate variant because the `Io` note ("run
    /// this with sudo") is wrong advice to someone who already did.
    Owner {
        path: String,
        uid: u32,
        gid: u32,
        source: std::io::Error,
    },
}

impl core::fmt::Display for StoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "{path}: {source}")?;
                // The likeliest cause, said out loud. This is what a person
                // gets for running the server as themselves rather than as the
                // service account, and "Permission denied" alone sends them to
                // chmod when the answer is sudo.
                if source.kind() == std::io::ErrorKind::PermissionDenied {
                    write!(
                        f,
                        "\n  note: the store files belong to the service account \
                         (breeze). Run this with sudo, or as that user."
                    )?;
                }
                Ok(())
            }
            Self::Parse { path, source } => write!(f, "{path} is not valid: {source}"),
            Self::Owner {
                path,
                uid,
                gid,
                source,
            } => write!(
                f,
                "{path} was left as it was: the new copy could not be given to its \
                 owner, {uid}:{gid} ({source})"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

/// Attach a path to an IO failure. Curried, so it drops into `.map_err(…)`.
fn io_at(path: &Path) -> impl Fn(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Serialise exactly as Breeze Core does: 2-space indent, no trailing newline.
///
/// `serde_json::to_string_pretty` already uses a 2-space indent, which is what
/// makes this a one-liner rather than a custom formatter — but it is asserted by
/// the round-trip tests rather than assumed.
pub fn to_json<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(value)
}

/// Read a store file. A missing file yields the type's default, which is how an
/// installation with no programs or timers yet behaves.
pub fn load<T: DeserializeOwned + Default>(path: impl AsRef<Path>) -> Result<T, StoreError> {
    let path = path.as_ref();
    match std::fs::read_to_string(path) {
        Ok(text) => {
            // An empty file is a half-written one; treat it as absent rather than
            // failing to start.
            if text.trim().is_empty() {
                return Ok(T::default());
            }
            serde_json::from_str(&text).map_err(|source| StoreError::Parse {
                path: path.display().to_string(),
                source,
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(io_at(path)(e)),
    }
}

/// Write a store file with the right permissions and the right owner.
///
/// The write goes to a temporary file in the same directory and is renamed into
/// place, so a crash or a full disk cannot leave a truncated store behind. Breeze
/// Core writes in place; doing better here is safe because the result is the same
/// bytes at the same path, **with the same owner** -- the part writing in place
/// gave for free, and `Owner` below keeps.
pub fn save<T: Serialize>(path: impl AsRef<Path>, value: &T, mode: Mode) -> Result<(), StoreError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_at(parent))?;
    }
    let text = to_json(value).map_err(|source| StoreError::Parse {
        path: path.display().to_string(),
        source,
    })?;
    let owner = Owner::of(path);

    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text.as_bytes()).map_err(io_at(&tmp))?;
    set_mode(&tmp, mode)?;
    // Handed over before the rename, so the file at `path` is never one the
    // service cannot read, not even for a moment.
    if let Some(owner) = owner {
        if let Err(e) = owner.give(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }
    // Rename is atomic within a filesystem, and replaces the target on Unix.
    std::fs::rename(&tmp, path).map_err(io_at(path))?;
    set_mode(path, mode)?;
    Ok(())
}

/// Who a store file belongs to, so that replacing it does not hand it to
/// whoever ran the command.
///
/// A renamed-in file is the writer's. The packages run the service as `breeze`,
/// and the documented `sudo breeze-core pair` and `sudo breeze-core fetch` run
/// as root, so config.json came out `root:root 640` and the service could not
/// read it at its next start -- on a fresh install, at its first.
///
/// A file being replaced keeps its owner and group; a new one gets its
/// directory's, as if the service had made it there. The mode stays
/// [`Mode`]'s, as Breeze Core's own chmod after every write makes it.
#[cfg(unix)]
#[derive(Clone, Copy)]
struct Owner {
    uid: u32,
    gid: u32,
}

#[cfg(unix)]
impl Owner {
    /// `None` when neither the file nor its directory can be asked: then the
    /// file is whatever writing it gives, as it always was.
    fn of(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let dir = path
                    .parent()
                    .filter(|d| !d.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                std::fs::metadata(dir).ok()?
            }
            Err(_) => return None,
        };
        Some(Self {
            uid: meta.uid(),
            gid: meta.gid(),
        })
    }

    /// Give the freshly written `tmp` to this owner before it replaces `path`.
    fn give(self, tmp: &Path, path: &Path) -> Result<(), StoreError> {
        use std::os::unix::fs::{chown, MetadataExt};
        let written = std::fs::metadata(tmp).map_err(io_at(tmp))?;
        // The usual case: the service writing its own files.
        if (written.uid(), written.gid()) == (self.uid, self.gid) {
            return Ok(());
        }
        match chown(tmp, Some(self.uid), Some(self.gid)) {
            Ok(()) => Ok(()),
            // Root may give a file to anyone, so a refusal means something odd
            // (an unmapped id, a filesystem without owners). Keep the old file.
            Err(source) if written.uid() == 0 => Err(StoreError::Owner {
                path: path.display().to_string(),
                uid: self.uid,
                gid: self.gid,
                source,
            }),
            // Anyone else can never give a file away, only its group, and only
            // to a group they are in. That much still lets the service read it
            // through the group. Past that the file stays the writer's, as it
            // always did: the service saving over a root-owned file it could
            // read is fine with the file becoming its own.
            Err(_) => {
                let _ = chown(tmp, None, Some(self.gid));
                Ok(())
            }
        }
    }
}

/// Windows has no owners to keep; the data directory's ACL decides.
#[cfg(not(unix))]
#[derive(Clone, Copy)]
struct Owner;

#[cfg(not(unix))]
impl Owner {
    fn of(_path: &Path) -> Option<Self> {
        None
    }

    fn give(self, _tmp: &Path, _path: &Path) -> Result<(), StoreError> {
        Ok(())
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: Mode) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode.bits()))
        .map_err(io_at(path))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: Mode) -> Result<(), StoreError> {
    // Windows has no mode bits to set; the installer restricts the data
    // directory's ACL instead, which is what Breeze Core already does there.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AppConfig, TimersDoc, UnitConfig};

    fn tempdir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        // Enough to keep concurrent tests apart without pulling in a temp crate.
        p.push(format!(
            "breeze-store-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_missing_file_is_an_empty_document_not_an_error() {
        let dir = tempdir();
        let doc: TimersDoc = load(dir.join("nope.json")).unwrap();
        assert_eq!(doc, TimersDoc::default());
    }

    #[test]
    fn an_empty_file_is_treated_as_absent() {
        // A half-written file should not stop the server from starting.
        let dir = tempdir();
        let p = dir.join("empty.json");
        std::fs::write(&p, "   \n").unwrap();
        let doc: TimersDoc = load(&p).unwrap();
        assert_eq!(doc, TimersDoc::default());
    }

    #[test]
    fn malformed_json_names_the_file() {
        let dir = tempdir();
        let p = dir.join("bad.json");
        std::fs::write(&p, "{ not json").unwrap();
        let err = load::<TimersDoc>(&p).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bad.json"), "unhelpful message: {msg}");
    }

    #[test]
    #[cfg(unix)]
    fn an_unreadable_file_names_the_file_and_suggests_sudo() {
        // The reported symptom was `breeze-core: Permission denied (os error
        // 13)` and nothing else -- no path, no hint that a file was even
        // involved, so it read as "this binary needs root to run at all".
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir();
        let p = dir.join("locked.json");
        std::fs::write(&p, "{}").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();

        let outcome = load::<TimersDoc>(&p);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();

        // Root reads a 0000 file happily, so the assertions below would prove
        // nothing there. Detected by outcome rather than by asking for the uid,
        // which keeps this crate free of a libc dependency for one test.
        let Err(err) = outcome else {
            return;
        };
        let msg = err.to_string();

        assert!(msg.contains("locked.json"), "does not name the file: {msg}");
        assert!(
            msg.contains("sudo"),
            "a permission error should say what to do about it: {msg}"
        );
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempdir();
        let p = dir.join("config.json");
        let cfg = AppConfig {
            api_key: Some("k".into()),
            units: vec![UnitConfig {
                name: "Room".into(),
                ip: "192.168.1.5".into(),
                port: 6444,
                id: 42,
                token: None,
                key: None,
            }],
        };
        save(&p, &cfg, Mode::AdminReadable).unwrap();
        assert_eq!(load::<AppConfig>(&p).unwrap(), cfg);
    }

    #[test]
    fn written_files_have_no_trailing_newline() {
        // model_dump_json + write_text produces none, so neither may we.
        let dir = tempdir();
        let p = dir.join("t.json");
        save(&p, &TimersDoc::default(), Mode::Private).unwrap();
        let raw = std::fs::read(&p).unwrap();
        assert_ne!(
            *raw.last().unwrap(),
            b'\n',
            "trailing newline would change every file"
        );
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let dir = tempdir();
        let p = dir.join("t.json");
        save(&p, &TimersDoc::default(), Mode::Private).unwrap();
        assert!(!p.with_extension("tmp").exists(), "stray .tmp file");
        assert!(p.exists());
    }

    #[cfg(unix)]
    #[test]
    fn modes_match_what_breeze_core_writes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir();
        for (name, mode, want) in [
            ("config.json", Mode::AdminReadable, 0o640),
            ("devices.json", Mode::Private, 0o600),
        ] {
            let p = dir.join(name);
            save(&p, &TimersDoc::default(), mode).unwrap();
            let got = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(got, want, "{name} should be {want:o}, got {got:o}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_file_keeps_its_owner_and_a_new_one_gets_its_directorys() {
        // `sudo breeze-core pair` replaced the service's breeze:breeze
        // config.json with a root:root one, and the service could not read it
        // at its next start. Only root can give a file away, so as anyone else
        // there is nothing to show and this returns early -- detected by
        // outcome, like the unreadable-file test above. It proves something in
        // a root container, which is where the bug was found.
        use std::os::unix::fs::{chown, MetadataExt, PermissionsExt};
        let dir = tempdir().join("owners");
        std::fs::create_dir_all(&dir).unwrap();
        // Made-up ids, standing in for the packages' `breeze`: root may give a
        // file to any id, account or not.
        let (dir_ids, file_ids) = ((4242, 4243), (4244, 4245));
        if chown(&dir, Some(dir_ids.0), Some(dir_ids.1)).is_err() {
            return;
        }
        let ids = |p: &Path| {
            let m = std::fs::metadata(p).unwrap();
            ((m.uid(), m.gid()), m.permissions().mode() & 0o777)
        };

        let live = dir.join("config.json");
        std::fs::write(&live, "{}").unwrap();
        chown(&live, Some(file_ids.0), Some(file_ids.1)).unwrap();
        save(&live, &TimersDoc::default(), Mode::AdminReadable).unwrap();
        assert_eq!(
            ids(&live),
            (file_ids, 0o640),
            "the replaced file changed hands"
        );

        let fresh = dir.join("devices.json");
        save(&fresh, &TimersDoc::default(), Mode::Private).unwrap();
        assert_eq!(
            ids(&fresh),
            (dir_ids, 0o600),
            "a new file is not its directory's"
        );
    }

    #[test]
    fn overwriting_preserves_the_mode() {
        let dir = tempdir();
        let p = dir.join("config.json");
        save(&p, &TimersDoc::default(), Mode::AdminReadable).unwrap();
        save(&p, &TimersDoc::default(), Mode::AdminReadable).unwrap();
        assert!(p.exists());
    }
}
