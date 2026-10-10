//! The only module of peekd that calls into libc directly (BLUEPRINT §1.3:
//! `unsafe` is confined to one module per binary). Each wrapper is a thin,
//! safe function around one system call, with the invariants spelled out.

#![allow(unsafe_code)]

use std::{
    ffi::CString,
    io,
    os::{fd::RawFd, unix::ffi::OsStrExt as _},
    path::{Path, PathBuf},
};

/// Sets the process umask to `0o077` so every file peekd creates is private
/// (§1.8 step 1). Returns the previous mask.
pub fn restrict_umask() -> u32 {
    // SAFETY: umask(2) only swaps the calling process's file-mode creation
    // mask; it cannot fail and touches no memory we own.
    u32::from(unsafe { libc::umask(0o077) })
}

/// The PID of the process on the other end of a connected Unix socket
/// (`getsockopt(SOL_LOCAL, LOCAL_PEERPID)`).
///
/// # Errors
/// The OS error when the option is unavailable (e.g. the peer has gone).
pub fn peer_pid(fd: RawFd) -> io::Result<i32> {
    let mut pid: libc::pid_t = 0;
    let mut len = libc::socklen_t::try_from(std::mem::size_of::<libc::pid_t>())
        .map_err(|_| io::Error::other("pid_t size does not fit socklen_t"))?;
    // SAFETY: `pid` and `len` are valid, writable and correctly sized for
    // LOCAL_PEERPID, which writes exactly one pid_t; `fd` is borrowed from a
    // live socket for the duration of the call.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            std::ptr::from_mut(&mut pid).cast::<libc::c_void>(),
            &raw mut len,
        )
    };
    if rc == 0 {
        Ok(pid)
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The executable path of a running process (`proc_pidpath`).
///
/// # Errors
/// The OS error when the process does not exist or is not inspectable.
pub fn pid_path(pid: i32) -> io::Result<PathBuf> {
    let size = usize::try_from(libc::PROC_PIDPATHINFO_MAXSIZE).unwrap_or(4096);
    let mut buf = vec![0u8; size];
    let cap = u32::try_from(buf.len()).unwrap_or(4096);
    // SAFETY: `buf` is a writable allocation of `cap` bytes that outlives the
    // call; proc_pidpath writes at most `cap` bytes and returns the length.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast::<libc::c_void>(), cap) };
    if n <= 0 {
        return Err(io::Error::last_os_error());
    }
    buf.truncate(usize::try_from(n).unwrap_or(0));
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&buf)))
}

fn c_path(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

/// Atomically swaps two paths on the same volume (`renamex_np(RENAME_SWAP)`),
/// the self-update primitive of D12: both must exist.
///
/// # Errors
/// The OS error (e.g. `EXDEV` across volumes, `ENOENT` when one is missing).
pub fn rename_swap(a: &Path, b: &Path) -> io::Result<()> {
    rename_np(a, b, libc::RENAME_SWAP)
}

fn rename_np(a: &Path, b: &Path, flags: libc::c_uint) -> io::Result<()> {
    let a = c_path(a)?;
    let b = c_path(b)?;
    // SAFETY: both arguments are NUL-terminated C strings that live until the
    // call returns; renamex_np does not retain them.
    let rc = unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), flags) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Sends SIGTERM to `pid` (used only for Peek.app's own PID, learned from the
/// verified UI connection, when it does not quit for an update in time).
///
/// # Errors
/// The OS error (`ESRCH` when the process is gone).
pub fn terminate(pid: i32) -> io::Result<()> {
    if pid <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to signal pid <= 1",
        ));
    }
    // SAFETY: kill(2) with a positive pid signals exactly that process; it
    // touches no memory.
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Whether a process exists (`kill(pid, 0)`).
#[must_use]
pub fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs only the existence and permission check.
    unsafe { libc::kill(pid, 0) == 0 }
}
