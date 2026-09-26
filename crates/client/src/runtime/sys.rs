//! The only place this crate talks to the OS about users: safe `nix`
//! wrappers on Unix, so the crate keeps `#![forbid(unsafe_code)]`.

use std::path::PathBuf;

use crate::error::{Error, ErrorCode, Result};

/// The real user ID (`getuid`).
#[cfg(unix)]
#[must_use]
pub fn uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

/// The effective user ID (`geteuid`); files peek creates are owned by it.
#[cfg(unix)]
#[must_use]
pub fn euid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

/// The user's home from the password database (`getpwuid_r`), never `$HOME`
/// and never `SILICON_HOME`.
///
/// # Errors
/// `invalid_silicon_home` when the account has no usable home.
#[cfg(unix)]
pub fn real_home() -> Result<PathBuf> {
    let uid = nix::unistd::getuid();
    match nix::unistd::User::from_uid(uid) {
        Ok(Some(user)) if !user.dir.as_os_str().is_empty() => Ok(user.dir),
        Ok(_) => Err(Error::new(
            ErrorCode::InvalidSiliconHome,
            format!("the password database has no home directory for uid {uid}"),
        )
        .with_hint("set SILICON_HOME to the Silicon's home directory")),
        Err(e) => Err(Error::new(
            ErrorCode::InvalidSiliconHome,
            format!("looking up the home directory of uid {uid} failed: {e}"),
        )
        .with_hint("set SILICON_HOME to the Silicon's home directory")),
    }
}

/// The user's home directory.
///
/// # Errors
/// `invalid_silicon_home` when it cannot be determined.
#[cfg(not(unix))]
pub fn real_home() -> Result<PathBuf> {
    ["USERPROFILE", "HOME"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .find(|p| p.is_absolute())
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidSiliconHome,
                "the user's home directory cannot be determined",
            )
            .with_hint("set SILICON_HOME to the Silicon's home directory")
        })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn ids_and_home() -> Result<()> {
        assert_eq!(uid(), euid());
        let home = real_home()?;
        assert!(home.is_absolute());
        Ok(())
    }
}
