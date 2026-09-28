//! Processes Syn starts go when Syn goes, however Syn goes.
//!
//! Helpers and `curl` were stopped by `Drop`, and `Drop` only runs on a
//! clean exit. Stop in the console kills a stuck CLI outright;
//! `Stop-Process`, which `scripts/console.ps1` tells people to use, kills
//! the console outright; Ctrl+C runs no destructors at all. Reported from
//! use as background processes left behind, and measured on Linux: a CLI
//! killed mid-request left its `curl` running, and a console killed while
//! its CLI worked left the CLI running its turn, spending tokens, for
//! nobody. `lo_host` already watched for its parent dying; office-host and
//! uia-host, on Windows, do not, and were left behind the same way.
//!
//! Three pieces, all std and a few system calls:
//!   - `bind`, before a spawn: on Linux the child is sent SIGTERM when the
//!     thread that started it dies (`PR_SET_PDEATHSIG`).
//!   - `adopt`, after a spawn: on Windows the child joins a job object that
//!     kills everything in it when the last handle to it closes, which the
//!     system does when this process ends, by any means.
//!   - `watch_parent`, in a CLI the console started: exit when the console
//!     is gone, so the two above fire.
//!
//! Never used for `shell`: a program the human approved, an application
//! among them, is theirs and must outlive Syn. And none of this reaches an
//! Office application, which COM starts outside this process tree; killing
//! a helper releases its hold and leaves the documents where they are.

use std::process::{Child, Command};

/// The console's process id, set on the CLI it starts.
pub const PARENT_ENV: &str = "SYN_PARENT_PID";

/// Before `spawn`: the child is to die with this process.
///
/// On Linux the signal is tied to the thread that spawns, not the process,
/// so call this only where the spawning thread outlives the child: the CLI
/// and the MCP server spawn from their main thread, and a thread that runs
/// `curl` waits for it.
pub fn bind(cmd: &mut Command) {
    imp::bind(cmd);
}

/// After `spawn`: see `bind`. Failure is ignored; the child still runs, and
/// the clean-exit path still stops it.
pub fn adopt(child: &Child) {
    imp::adopt(child);
}

/// In a process started by the console: exit once the console has gone.
/// Does nothing when `SYN_PARENT_PID` is unset, which is every CLI started
/// by hand.
pub fn watch_parent() {
    let Some(pid) = std::env::var(PARENT_ENV).ok().and_then(|p| p.trim().parse::<u32>().ok()) else {
        return;
    };
    imp::watch(pid);
}

#[cfg(target_os = "linux")]
mod imp {
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    unsafe extern "C" {
        fn prctl(option: i32, ...) -> i32;
        fn getppid() -> i32;
    }
    const PR_SET_PDEATHSIG: i32 = 1;
    const SIGTERM: std::ffi::c_ulong = 15;
    const ESRCH: i32 = 3;

    pub fn bind(cmd: &mut Command) {
        let parent = std::process::id() as i32;
        // SAFETY: between fork and exec only async-signal-safe calls may be
        // made; prctl and getppid are bare system calls and allocate nothing.
        unsafe {
            cmd.pre_exec(move || {
                if prctl(PR_SET_PDEATHSIG, SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Died between the fork and the prctl: the signal will never
                // come, so do not start at all.
                if getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(ESRCH));
                }
                Ok(())
            });
        }
    }

    pub fn adopt(_child: &Child) {}

    pub fn watch(pid: u32) {
        super::unix_watch(pid, || unsafe { getppid() } as u32);
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
mod imp {
    use std::process::{Child, Command};

    unsafe extern "C" {
        fn getppid() -> i32;
    }

    /// No parent-death signal here; the clean exit still stops helpers.
    pub fn bind(_cmd: &mut Command) {}

    pub fn adopt(_child: &Child) {}

    pub fn watch(pid: u32) {
        super::unix_watch(pid, || unsafe { getppid() } as u32);
    }
}

/// A process whose parent dies is handed to another: a changed parent id is
/// the parent's death, whatever became of it.
#[cfg(unix)]
fn unix_watch(pid: u32, parent: fn() -> u32) {
    std::thread::spawn(move || {
        loop {
            if parent() != pid {
                std::process::exit(0);
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Command};
    use std::sync::OnceLock;

    // std, not core: this crate is itself named `core` (see desk.rs).
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(job: *mut c_void, class: i32, info: *const c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, ms: u32) -> u32;
    }

    /// JOBOBJECT_BASIC_LIMIT_INFORMATION, field for field.
    #[repr(C)]
    #[derive(Default)]
    struct Basic {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    /// JOBOBJECT_EXTENDED_LIMIT_INFORMATION: the basic limits, IO_COUNTERS
    /// (six u64s), and four sizes.
    #[repr(C)]
    #[derive(Default)]
    struct Extended {
        basic: Basic,
        io: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    const EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const INFINITE: u32 = 0xFFFF_FFFF;

    /// One job for the life of the process, its handle never closed: the
    /// system closes it at exit, and that is the moment it is for. Kept as
    /// an integer because a raw pointer is not `Sync`.
    fn job() -> Option<*mut c_void> {
        static JOB: OnceLock<usize> = OnceLock::new();
        let h = *JOB.get_or_init(|| {
            // SAFETY: plain Win32 calls on a structure laid out as the
            // headers declare it; a failure leaves 0 and adoption off.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if job.is_null() {
                    return 0;
                }
                let mut info = Extended::default();
                info.basic.limit_flags = KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job,
                    EXTENDED_LIMIT_INFORMATION,
                    (&info as *const Extended).cast(),
                    std::mem::size_of::<Extended>() as u32,
                );
                if ok == 0 { 0 } else { job as usize }
            }
        });
        (h != 0).then_some(h as *mut c_void)
    }

    pub fn bind(_cmd: &mut Command) {}

    pub fn adopt(child: &Child) {
        if let Some(job) = job() {
            // SAFETY: the child's handle is valid while `child` is borrowed.
            unsafe { AssignProcessToJobObject(job, child.as_raw_handle().cast()) };
        }
    }

    #[test]
    fn the_limit_structure_is_the_size_windows_declares() {
        // sizeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION): a wrong layout would
        // have SetInformationJobObject refuse it, and adoption would be off
        // without a word.
        let want = if cfg!(target_pointer_width = "64") { 144 } else { 112 };
        assert_eq!(std::mem::size_of::<Extended>(), want);
        assert!(job().is_some(), "the job object could not be made");
    }

    pub fn watch(pid: u32) {
        std::thread::spawn(move || {
            // SAFETY: a handle opened, waited on, and left to the system.
            unsafe {
                let h = OpenProcess(SYNCHRONIZE, 0, pid);
                // Already gone by the time this looked: go too.
                if !h.is_null() {
                    WaitForSingleObject(h, INFINITE);
                }
            }
            std::process::exit(0);
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_bound_child_still_runs_and_reports_normally() {
        // What `bind` and `adopt` must never do is get in the way of the
        // child they are tying down.
        let mut cmd = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "echo tethered"]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "echo tethered"]);
            c
        };
        cmd.stdout(std::process::Stdio::piped());
        super::bind(&mut cmd);
        let child = cmd.spawn().expect("spawn");
        super::adopt(&child);
        let out = child.wait_with_output().expect("wait");
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("tethered"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_bound_child_dies_with_the_process_that_started_it() {
        // The kernel ties the signal to the thread that spawned the child,
        // which is what makes this observable from a test: bind a child from
        // a thread that then ends, and it is signalled exactly as it would
        // be when the CLI that started it died.
        let child = std::thread::spawn(|| {
            let mut cmd = std::process::Command::new("sleep");
            cmd.arg("30");
            super::bind(&mut cmd);
            cmd.spawn().expect("spawn")
        })
        .join()
        .expect("thread");
        let mut child = child;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                assert!(!status.success(), "the child should have been signalled, not finished");
                break;
            }
            assert!(std::time::Instant::now() < until, "the child outlived the thread that bound it");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
