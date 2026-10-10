//! The per-account layout under `~/Library/Application Support/Peek/`
//! (BLUEPRINT §1.7, amendment §0.1 item 7). Directories are 0700 and files
//! 0600; `~` is the getpwuid home, never `SILICON_HOME`.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use silicon_peek_client::{
    Error, Result,
    identity::{AccountId, Context},
    runtime::fs::ensure_private_dir,
};

/// Every path peekd owns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// The support directory itself.
    pub support: PathBuf,
}

impl Paths {
    /// The layout rooted at `support`.
    #[must_use]
    pub fn new(support: &Path) -> Self {
        Self {
            support: support.to_path_buf(),
        }
    }

    /// Creates every directory (0700), including missing parents of the
    /// support directory.
    ///
    /// # Errors
    /// `invalid_silicon_home` for a symlink, a foreign owner or an I/O error.
    pub fn ensure(&self) -> Result<()> {
        if let Some(parent) = self.support.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::internal(format!("creating {} failed: {e}", parent.display()))
                    .with_hint("check that your home directory is writable")
            })?;
        }
        ensure_private_dir(&self.support)?;
        for d in [
            self.drawings_root(),
            self.support.join("cache"),
            self.images_dir(),
            self.tts_dir(),
            self.recordings_dir(),
            self.offers_dir(),
        ] {
            ensure_private_dir(&d)?;
        }
        Ok(())
    }

    /// `peekd.sqlite`.
    #[must_use]
    pub fn db(&self) -> PathBuf {
        self.support.join("peekd.sqlite")
    }

    /// `settings.json`.
    #[must_use]
    pub fn settings(&self) -> PathBuf {
        self.support.join("settings.json")
    }

    /// `homes.json`.
    #[must_use]
    pub fn homes_json(&self) -> PathBuf {
        self.support.join("homes.json")
    }

    /// `peekd.log`.
    #[must_use]
    pub fn log(&self) -> PathBuf {
        self.support.join("peekd.log")
    }

    /// `drawings/`.
    #[must_use]
    pub fn drawings_root(&self) -> PathBuf {
        self.support.join("drawings")
    }

    /// `drawings/<context>/<account-uuid>/`.
    #[must_use]
    pub fn drawing_dir(&self, context: Context, account: &AccountId) -> PathBuf {
        self.drawings_root()
            .join(context.as_string())
            .join(account.as_str())
    }

    /// Creates `drawings/<context>/<account-uuid>/` (each level 0700).
    ///
    /// # Errors
    /// As [`Paths::ensure`].
    pub fn ensure_drawing_dir(&self, context: Context, account: &AccountId) -> Result<PathBuf> {
        let mut dir = self.drawings_root();
        for part in [context.as_string(), account.as_str().to_owned()] {
            dir.push(part);
            ensure_private_dir(&dir)?;
        }
        Ok(dir)
    }

    /// `cache/images/`.
    #[must_use]
    pub fn images_dir(&self) -> PathBuf {
        self.support.join("cache/images")
    }

    /// `cache/tts/`.
    #[must_use]
    pub fn tts_dir(&self) -> PathBuf {
        self.support.join("cache/tts")
    }

    /// `recordings/`.
    #[must_use]
    pub fn recordings_dir(&self) -> PathBuf {
        self.support.join("recordings")
    }

    /// `recordings/<id>.wav`.
    #[must_use]
    pub fn recording(&self, id: &str) -> PathBuf {
        self.recordings_dir().join(format!("{id}.wav"))
    }

    /// `offers/`.
    #[must_use]
    pub fn offers_dir(&self) -> PathBuf {
        self.support.join("offers")
    }

    /// `rejected.json`.
    #[must_use]
    pub fn rejected(&self) -> PathBuf {
        self.support.join("rejected.json")
    }

    /// `install.lock`.
    #[must_use]
    pub fn install_lock(&self) -> PathBuf {
        self.support.join("install.lock")
    }

    /// `install-status.txt`.
    #[must_use]
    pub fn install_status(&self) -> PathBuf {
        self.support.join("install-status.txt")
    }

    /// `update-applied.json`.
    #[must_use]
    pub fn update_applied(&self) -> PathBuf {
        self.support.join("update-applied.json")
    }
}

/// Lowercase hex SHA-256.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
