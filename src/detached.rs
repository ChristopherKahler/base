//! Starting a process nobody waits for (BO-26: U1's upgrade, and U7 under lynx's G1 ruling).
//!
//! A hook starts work it must not wait for: the upgrade, the auto-update's download, a code-map build. Each child gets
//! no stdin, stdout or stderr, so nothing it does prints into a session. That is not enough on Windows: `CreateProcess`
//! hands a child every inheritable handle of its parent, and the pipes Claude Code gives a hook for its stdin, stdout
//! and stderr are inheritable. The child held the hook's stdout open, and Claude Code, reading until it closes, waited
//! for the child as if it were the hook. Measured on 2026-10-04 with a code-map build of about 2 s: the hook exited at
//! 0.05 s and its stdout reached end of file at 1.87 to 1.94 s (`base-0160-measure/bo26_pipe_wait.py`).
//!
//! [`spawn`] keeps this process's own three standard handles out of the child: on Windows their inherit flag is cleared
//! for the moment of the spawn and put back after it. On Unix the child's descriptors 0 to 2 are `/dev/null`, which
//! closes its copies of the pipes, so nothing more is needed.

use std::process::{Child, Command, Stdio};

/// Start `cmd` with no stdin, stdout or stderr (on Windows also with no console window, and without this process's own
/// standard handles), and return without waiting for it.
pub fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: a console child of a windowless hook would otherwise open one.
        cmd.creation_flags(0x0800_0000);
        std_handles_kept_back(|| cmd.spawn())
    }
    #[cfg(not(windows))]
    cmd.spawn()
}

/// Run `f` with this process's standard handles not inheritable, then put their flags back as they were. The flag is
/// process-wide, so one lock covers clear, spawn and restore: two threads spawning at once would otherwise put
/// inheritance back under each other (lynx's review of U7). `Command::spawn`'s own duplication of a handle for
/// `Stdio::inherit` is unaffected.
#[cfg(windows)]
fn std_handles_kept_back<T>(f: impl FnOnce() -> T) -> T {
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn GetHandleInformation(handle: *mut c_void, flags: *mut u32) -> i32;
        fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 1;
    // STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE: (DWORD)-10, -11, -12.
    let mut cleared: Vec<*mut c_void> = Vec::new();
    for which in [-10i32, -11, -12] {
        // SAFETY: plain kernel32 calls on this process's own standard handles. GetStdHandle returns null or
        // INVALID_HANDLE_VALUE (-1) when there is none, and those are skipped; the other two read and change one flag.
        unsafe {
            let handle = GetStdHandle(which as u32);
            if handle.is_null() || handle as isize == -1 || cleared.contains(&handle) {
                continue;
            }
            let mut flags = 0u32;
            if GetHandleInformation(handle, &mut flags) != 0
                && flags & HANDLE_FLAG_INHERIT != 0
                && SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) != 0
            {
                cleared.push(handle);
            }
        }
    }
    let out = f();
    for handle in cleared {
        // SAFETY: as above; puts back the flag this function cleared.
        unsafe {
            SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
        }
    }
    out
}
