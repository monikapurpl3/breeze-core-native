//! Saving a file the CLI generated: a `diag --nerd` snapshot, a backup.

use std::path::{Path, PathBuf};

/// Create (or replace) a file readable only by its owner.
///
/// Both things saved through here can be sensitive -- a backup holds every key
/// the server has, a nerd snapshot names its addresses and paths -- so neither
/// is ever written readable by others, even for a moment.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// Save a file under a generated `name` in the current directory -- or, where
/// that cannot be written, in the home folder, saying so.
///
/// The current directory is wherever the shell happened to be, and that is
/// often somewhere nobody can write: `C:\Program Files\PowerShell\7` is where
/// an ssh session into Windows starts, and `/` is where anything a systemd
/// service runs does. Failing there with "Permission denied" is a correct
/// answer to a question nobody asked. A name given explicitly never falls
/// back: that was a decision, not a default.
pub fn save_here_or_home(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let here = PathBuf::from(name);
    let first = match write_private(&here, bytes) {
        Ok(()) => return Ok(here),
        Err(e) => e,
    };
    let Some(home) = home_dir() else {
        return Err(format!("cannot write {name} here: {first}"));
    };
    let there = home.join(name);
    match write_private(&there, bytes) {
        Ok(()) => {
            println!(
                "  (this directory is not writable: {first}; saved in the home folder instead)"
            );
            Ok(there)
        }
        Err(second) => Err(format!(
            "cannot write {name} here ({first}) or in {} ({second})",
            home.display()
        )),
    }
}

/// The home folder, from the environment the way every shell sets it.
fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// A name for a generated file: letters, digits and `-`, everything else `_`,
/// so a hostname with dots or spaces makes a name every shell takes unquoted.
pub fn tidy(part: &str) -> String {
    part.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_tidied_for_any_shell() {
        assert_eq!(tidy("living-room.lan"), "living-room_lan");
        assert_eq!(tidy("Monika's PC"), "Monika_s_PC");
    }

    #[cfg(unix)]
    #[test]
    fn saved_files_are_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let path = std::env::temp_dir().join(format!("breeze-private-{}", std::process::id()));
        write_private(&path, b"secret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_file(&path);
        assert_eq!(mode, 0o600);
    }
}
