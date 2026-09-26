//! The only module of the CLI that uses `unsafe` (BLUEPRINT §1.3): two libc
//! calls with no safe wrapper in std.
//!
//! - [`rename_exclusive`]: `renamex_np(from, to, RENAME_EXCL)` installs the
//!   first Peek.app atomically and never over an existing bundle (§1.8).
//! - [`detach`]: `setsid()` in the child before `exec`, so a background helper
//!   (`peek __after-login`) leaves the caller's session and process group and
//!   survives the terminal that started `peek login`.
#![allow(unsafe_code)]

/// Atomically renames `from` to `to`, failing with `EEXIST` if `to` exists.
///
/// # Errors
/// The OS error: `EEXIST` when `to` exists, `EXDEV` across volumes, and so on.
#[cfg(target_os = "macos")]
pub fn rename_exclusive(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt as _};

    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both arguments are valid NUL-terminated C strings owned by this
    // frame and alive for the whole call; renamex_np does not retain them.
    let rc = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Makes `command`'s child start a new session (`setsid`) before `exec`.
#[cfg(target_os = "macos")]
pub fn detach(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt as _;

    // SAFETY: the closure runs in the forked child before exec. It only calls
    // setsid(2), which is async-signal-safe, and allocates nothing. The child
    // is not a process-group leader (no process_group() is set), so setsid
    // cannot fail with EPERM for that reason.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn exclusive_rename_never_replaces() -> std::io::Result<()> {
        let t = tempfile::tempdir()?;
        let a = t.path().join("a");
        let b = t.path().join("b");
        std::fs::create_dir(&a)?;
        rename_exclusive(&a, &b)?;
        assert!(b.is_dir() && !a.exists());
        std::fs::create_dir(&a)?;
        let e = rename_exclusive(&a, &b).err();
        assert_eq!(e.and_then(|e| e.raw_os_error()), Some(libc::EEXIST));
        assert!(a.is_dir(), "the source stays when the target exists");
        Ok(())
    }
}
