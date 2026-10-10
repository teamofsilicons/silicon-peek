//! Private file primitives for the store (BLUEPRINT §1.7): directories 0700,
//! files 0600 from creation, symlinks refused (`O_NOFOLLOW`), foreign owners
//! refused, atomic replacement (temp + `sync_all` + `rename` + directory
//! fsync) and exclusive creation (`O_EXCL`).

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read as _, Write as _},
    path::Path,
};

use uuid::Uuid;

use crate::error::{Error, ErrorCode, Result};

/// Largest store file read into memory.
pub const MAX_STORE_FILE_BYTES: u64 = 4 * 1024 * 1024;

fn io_error(action: &str, path: &Path, e: &io::Error) -> Error {
    let code = match e.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::NotADirectory => {
            ErrorCode::InvalidSiliconHome
        }
        _ => ErrorCode::InternalError,
    };
    Error::new(code, format!("{action} {} failed: {e}", path.display())).with_hint(format!(
        "check that {} exists, is owned by you, and that the disk has free space",
        path.parent().unwrap_or(path).display()
    ))
}

fn symlink_refused(path: &Path) -> Error {
    Error::new(
        ErrorCode::InvalidSiliconHome,
        format!(
            "{} is a symbolic link; peek refuses symlinks in its store",
            path.display()
        ),
    )
    .with_hint("replace the link with a real directory or file owned by you")
}

#[cfg(unix)]
fn is_loop(e: &io::Error) -> bool {
    e.raw_os_error() == Some(nix::errno::Errno::ELOOP as i32)
}

#[cfg(unix)]
fn nofollow() -> i32 {
    nix::fcntl::OFlag::O_NOFOLLOW.bits()
}

#[cfg(unix)]
fn check_owner(path: &Path, meta: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    let me = super::sys::euid();
    if meta.uid() != me {
        return Err(Error::new(
            ErrorCode::InvalidSiliconHome,
            format!(
                "{} is owned by uid {}, not by you (uid {me}); peek refuses stores other accounts can control",
                path.display(),
                meta.uid()
            ),
        )
        .with_hint("run peek as the account that owns SILICON_HOME, or fix the ownership"));
    }
    Ok(())
}

/// Creates (if needed) and verifies a private directory: not a symlink, owned
/// by this user, mode tightened to 0700. The parent must exist.
///
/// # Errors
/// `invalid_silicon_home` for a symlink, a foreign owner, a non-directory or
/// a missing parent.
pub fn ensure_private_dir(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => return Err(symlink_refused(path)),
        Ok(m) if !m.is_dir() => {
            return Err(Error::new(
                ErrorCode::InvalidSiliconHome,
                format!("{} exists but is not a directory", path.display()),
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut b = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                b.mode(0o700);
            }
            match b.create(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io_error("creating", path, &e)),
            }
        }
        Err(e) => return Err(io_error("inspecting", path, &e)),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(nix::fcntl::OFlag::O_DIRECTORY.bits() | nofollow())
            .open(path)
            .map_err(|e| {
                if is_loop(&e) {
                    symlink_refused(path)
                } else {
                    io_error("opening", path, &e)
                }
            })?;
        let meta = dir
            .metadata()
            .map_err(|e| io_error("inspecting", path, &e))?;
        check_owner(path, &meta)?;
        if meta.permissions().mode() & 0o077 != 0 {
            dir.set_permissions(fs::Permissions::from_mode(0o700))
                .map_err(|e| io_error("restricting", path, &e))?;
        }
    }
    Ok(())
}

/// Verifies, without changing anything, that a directory is private: a real
/// directory owned by this user with `mode & 0o077 == 0` (peekd's check 1).
///
/// # Errors
/// `invalid_silicon_home` naming the failed rule.
pub fn verify_private_dir(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).map_err(|e| io_error("inspecting", path, &e))?;
    if meta.file_type().is_symlink() {
        return Err(symlink_refused(path));
    }
    if !meta.is_dir() {
        return Err(Error::new(
            ErrorCode::InvalidSiliconHome,
            format!("{} is not a directory", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        check_owner(path, &meta)?;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(Error::new(
                ErrorCode::InvalidSiliconHome,
                format!(
                    "{} has mode {mode:03o}; a peek store must be private (0700)",
                    path.display()
                ),
            )
            .with_hint(format!("chmod 700 {}", path.display())));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn check_file(path: &Path, file: &File) -> Result<fs::Metadata> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let meta = file
        .metadata()
        .map_err(|e| io_error("inspecting", path, &e))?;
    if !meta.is_file() {
        return Err(Error::new(
            ErrorCode::InvalidSiliconHome,
            format!("{} is not a regular file", path.display()),
        ));
    }
    check_owner(path, &meta)?;
    if meta.nlink() > 1 {
        return Err(Error::new(
            ErrorCode::InvalidSiliconHome,
            format!(
                "{} has {} hard links; peek refuses hard-linked store files",
                path.display(),
                meta.nlink()
            ),
        )
        .with_hint("remove the extra links, or delete the file and log in again"));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| io_error("restricting", path, &e))?;
    }
    Ok(meta)
}

/// Reads a private file; `None` when it does not exist.
///
/// # Errors
/// `invalid_silicon_home` for a symlink, a foreign owner or hard links;
/// `store_corrupt` for an oversized file.
pub fn read_private(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut o = OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        o.custom_flags(nofollow());
    }
    #[cfg(not(unix))]
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(symlink_refused(path));
    }
    let mut file = match o.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        #[cfg(unix)]
        Err(e) if is_loop(&e) => return Err(symlink_refused(path)),
        Err(e) => return Err(io_error("opening", path, &e)),
    };
    #[cfg(unix)]
    let len = check_file(path, &file)?.len();
    #[cfg(not(unix))]
    let len = file
        .metadata()
        .map_err(|e| io_error("inspecting", path, &e))?
        .len();
    if len > MAX_STORE_FILE_BYTES {
        return Err(Error::new(
            ErrorCode::StoreCorrupt,
            format!(
                "{} is {len} bytes; store files are at most {MAX_STORE_FILE_BYTES}",
                path.display()
            ),
        ));
    }
    let mut out = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.read_to_end(&mut out)
        .map_err(|e| io_error("reading", path, &e))?;
    Ok(Some(out))
}

fn new_private_file(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        o.mode(0o600).custom_flags(nofollow());
    }
    o.open(path)
}

#[cfg_attr(not(unix), allow(clippy::unnecessary_wraps))] // fallible on Unix only
fn sync_dir(dir: &Path) -> Result<()> {
    // Windows cannot open a directory as a file, and its rename is durable.
    #[cfg(unix)]
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| io_error("syncing", dir, &e))?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Atomically replaces `dir/name` with `bytes`: a 0600 temp file created with
/// `O_EXCL`, written, `sync_all`ed, renamed over the target, then the
/// directory is fsynced. The temp file is removed on failure.
///
/// # Errors
/// I/O failures, naming the path.
pub fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let target = dir.join(name);
    let temp = dir.join(format!(".{name}.{}.tmp", Uuid::now_v7().simple()));
    let result = (|| -> Result<()> {
        let mut f = new_private_file(&temp).map_err(|e| io_error("creating", &temp, &e))?;
        f.write_all(bytes)
            .and_then(|()| f.sync_all())
            .map_err(|e| io_error("writing", &temp, &e))?;
        drop(f);
        fs::rename(&temp, &target).map_err(|e| io_error("replacing", &target, &e))?;
        sync_dir(dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Creates `path` with `bytes` only if it does not exist (`O_EXCL`, 0600,
/// synced). Returns `false` when it already existed.
///
/// # Errors
/// I/O failures other than "already exists".
pub fn create_exclusive(path: &Path, bytes: &[u8]) -> Result<bool> {
    let mut f = match new_private_file(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(io_error("creating", path, &e)),
    };
    let written = f.write_all(bytes).and_then(|()| f.sync_all());
    if let Err(e) = written {
        drop(f);
        let _ = fs::remove_file(path);
        return Err(io_error("writing", path, &e));
    }
    if let Some(dir) = path.parent() {
        sync_dir(dir)?;
    }
    Ok(true)
}

/// Opens (creating if needed, never truncating) a 0600 lock file.
///
/// # Errors
/// `invalid_silicon_home` for a symlink or a foreign owner.
pub fn open_lock_file(path: &Path) -> Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        o.mode(0o600).custom_flags(nofollow());
    }
    #[cfg(not(unix))]
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(symlink_refused(path));
    }
    let file = o.open(path).map_err(|e| {
        #[cfg(unix)]
        if is_loop(&e) {
            return symlink_refused(path);
        }
        io_error("opening", path, &e)
    })?;
    #[cfg(unix)]
    check_file(path, &file)?;
    Ok(file)
}

/// Removes a file; a missing file is not an error.
///
/// # Errors
/// Other I/O failures.
pub fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error("removing", path, &e)),
    }
}
