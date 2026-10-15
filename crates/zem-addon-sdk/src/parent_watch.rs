//! Parent (host) death detection.
//!
//! Add-ons are spawned as direct children of the zem daemon. Nothing signals an
//! addon when the daemon crashes or is restarted, so without this the addon
//! process would be orphaned and keep running (and re-connecting) forever.
//!
//! We therefore watch the parent process and terminate the add-on the moment it
//! exits. Detection is immediate on the two supported platforms and works even
//! on hard crashes/SIGKILL of the host:
//!
//! - Linux: `prctl(PR_SET_PDEATHSIG, SIGTERM)` — the kernel delivers `SIGTERM`
//!   when the parent dies.
//! - macOS: a `kqueue` `EVFILT_PROC`/`NOTE_EXIT` watch on the parent PID.
//! - Other platforms: a fallback that polls `getppid()` (which is re-parented to
//!   PID 1 when the parent exits).

use std::io;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use std::time::Duration;

/// Start watching the parent process and terminate this process when it dies.
/// Safe to call once, early in addon startup.
pub(crate) fn start() {
    #[cfg(target_os = "linux")]
    install_pdeathsig();

    #[cfg(target_os = "macos")]
    watch_parent_kqueue();

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    watch_parent_poll();
}

#[cfg(target_os = "linux")]
fn install_pdeathsig() {
    let r = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) };
    if r != 0 {
        tracing::warn!(
            "prctl(PR_SET_PDEATHSIG) failed: {}",
            io::Error::last_os_error()
        );
        return;
    }
    // Race: if the parent died between spawn and this call the signal may be
    // missed because there is no parent left to signal us. Being re-parented to
    // PID 1 is the reliable sign the parent is already gone.
    let ppid = unsafe { libc::getppid() };
    if ppid <= 1 {
        tracing::warn!("host already exited during addon startup; terminating");
        std::process::exit(0);
    }
}

#[cfg(target_os = "macos")]
fn watch_parent_kqueue() {
    std::thread::spawn(move || {
        unsafe {
            let kq = libc::kqueue();
            if kq < 0 {
                tracing::warn!("kqueue() failed: {}", io::Error::last_os_error());
                return;
            }
            let parent = libc::getppid();
            let mut changes: libc::kevent = std::mem::zeroed();
            changes.ident = parent as libc::uintptr_t;
            changes.filter = libc::EVFILT_PROC;
            changes.flags = libc::EV_ADD | libc::EV_RECEIPT;
            changes.fflags = libc::NOTE_EXIT;

            let mut receipts = [std::mem::zeroed::<libc::kevent>(); 1];
            let r = libc::kevent(
                kq,
                &changes as *const libc::kevent,
                1,
                receipts.as_mut_ptr(),
                receipts.len() as i32,
                std::ptr::null(),
            );
            if r < 0 {
                tracing::warn!("kevent(register) failed: {}", io::Error::last_os_error());
                return;
            }

            // Race: if the parent died before the knote was registered, the
            // `NOTE_EXIT` event may never fire. Being re-parented to PID 1 is
            // the reliable sign the host is already gone.
            if libc::getppid() <= 1 {
                tracing::warn!("host already exited during addon startup; terminating");
                std::process::exit(0);
            }

            // Block until the parent exits. If it was already gone at
            // registration, NOTE_EXIT fires immediately.
            loop {
                let mut ev = [std::mem::zeroed::<libc::kevent>(); 1];
                let n = libc::kevent(
                    kq,
                    std::ptr::null(),
                    0,
                    ev.as_mut_ptr(),
                    ev.len() as i32,
                    std::ptr::null(),
                );
                if n < 0 {
                    tracing::warn!("kevent(wait) failed: {}", io::Error::last_os_error());
                    return;
                }
                if n > 0 {
                    tracing::info!("host process exited; terminating addon");
                    std::process::exit(0);
                }
            }
        }
    });
}

/// Fallback for platforms without `PR_SET_PDEATHSIG` or `EVFILT_PROC`: poll
/// `getppid()`. Once orphaned, the child is re-parented to PID 1.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn watch_parent_poll() {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let ppid = unsafe { libc::getppid() };
        if ppid <= 1 {
            tracing::info!("host process exited; terminating addon");
            std::process::exit(0);
        }
    });
}
