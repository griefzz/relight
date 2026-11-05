//! Making sure the ffmpeg processes relight starts end with it, and that an update's installer
//! doesn't.

/// The job relight got into, if it did.
#[cfg(windows)]
static JOB: std::sync::atomic::AtomicPtr<std::ffi::c_void> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

#[cfg(windows)]
fn set_limits(job: windows_sys::Win32::Foundation::HANDLE, flags: u32) -> std::io::Result<()> {
    use windows_sys::Win32::System::JobObjects::{
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    // SAFETY: a plain Win32 call on a job handle kept open for as long as the process runs,
    // given a zeroed struct of the size it's told.
    let set = unsafe {
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = flags;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if set == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

/// Puts this process in a Windows job that closes when it exits, taking every ffmpeg it
/// started with it, so quitting or crashing mid-export doesn't leave one running.
#[cfg(windows)]
pub fn end_children_on_exit() {
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: plain Win32 calls on a job handle made here; the handle is left open on
    // purpose, for the OS to close when the process ends.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return;
        }
        if set_limits(job, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE).is_ok() && AssignProcessToJobObject(job, GetCurrentProcess()) != 0 {
            JOB.store(job, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

/// Whether what relight starts from now on stays out of its job, and so outlives it. On for
/// an update's installer, which the updater starts just before ending relight: in the job, it
/// would end with relight, before it had done anything. What's already running still ends with
/// relight, which frees ffmpeg's files for the installer to replace.
#[cfg(windows)]
#[tauri::command]
pub fn let_children_outlive(on: bool) -> Result<(), String> {
    use windows_sys::Win32::System::JobObjects::{JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK};

    let job = JOB.load(std::sync::atomic::Ordering::SeqCst);
    if job.is_null() {
        // There's no job to end anything.
        return Ok(());
    }
    let flags = if on {
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK
    } else {
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    };
    set_limits(job, flags).map_err(|e| format!("Couldn't get ready to install the update: {e}"))
}

#[cfg(not(windows))]
pub fn end_children_on_exit() {}

#[cfg(not(windows))]
#[tauri::command]
pub fn let_children_outlive(_on: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use std::env;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    /// Set for the copy of the test that plays relight: where it's to leave its marks.
    const MARKS: &str = "RELIGHT_TEST_MARKS";

    /// Starts something that leaves a file after some seconds, if it's still running by then.
    #[expect(clippy::zombie_processes, reason = "whether it outlives what started it is the test")]
    fn start_marking(file: &Path, seconds: u32) {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // ping waits a second between pings.
        Command::new("cmd")
            .raw_arg(format!("/c ping -n {} 127.0.0.1 >nul & echo alive> \"{}\"", seconds + 1, file.display()))
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
    }

    /// Waits for a mark that should come, within reason.
    fn wait_for(file: &Path, otherwise: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !file.exists() {
            assert!(Instant::now() < deadline, "{otherwise}");
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Exiting is the point, so the test `name` runs another copy of itself, which does
    /// `relight` in a job and exits. Gives back the folder for its marks, in the first copy.
    fn play_relight(name: &str, relight: impl FnOnce(&Path)) -> tempfile::TempDir {
        if let Some(dir) = env::var_os(MARKS) {
            end_children_on_exit();
            relight(Path::new(&dir));
            std::process::exit(0);
        }
        let dir = tempfile::tempdir().unwrap();
        let played = Command::new(env::current_exe().unwrap())
            .args(["--exact", name])
            .env(MARKS, dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(played.success());
        dir
    }

    #[test]
    fn the_installer_outlives_relight_and_ffmpeg_doesnt() {
        let dir = play_relight("process::tests::the_installer_outlives_relight_and_ffmpeg_doesnt", |dir| {
            start_marking(&dir.join("ffmpeg"), 2);
            let_children_outlive(true).unwrap();
            start_marking(&dir.join("installer"), 4);
        });
        wait_for(&dir.path().join("installer"), "the installer ended with relight");
        assert!(!dir.path().join("ffmpeg").exists(), "an ffmpeg outlived relight");
    }

    #[test]
    fn after_a_failed_install_everything_ends_with_relight_again() {
        let dir = play_relight("process::tests::after_a_failed_install_everything_ends_with_relight_again", |dir| {
            let_children_outlive(true).unwrap();
            let_children_outlive(false).unwrap();
            start_marking(&dir.join("ffmpeg"), 2);
        });
        // Started here, outside the job, so it's there to say when the time's up.
        start_marking(&dir.path().join("time up"), 4);
        wait_for(&dir.path().join("time up"), "the time should be up");
        assert!(!dir.path().join("ffmpeg").exists(), "an ffmpeg outlived relight");
    }
}
