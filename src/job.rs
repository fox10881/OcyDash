//! Child-process containment via a Win32 Job Object with
//! KILL_ON_JOB_CLOSE: any spawned child assigned to the job dies when
//! this process does — including `taskkill /F` and crashes, where no
//! Drop impl ever runs (the orphaned ffmpeg/SystemInfos procs that used
//! to accumulate across restarts).

use std::sync::OnceLock;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Threading::OpenProcess;

static JOB: OnceLock<usize> = OnceLock::new();

/// 0 = unavailable, else the job HANDLE as usize (HANDLE isn't Sync).
fn job() -> usize {
    *JOB.get_or_init(|| unsafe {
        let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if h.is_null() {
            return 0;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            h,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            CloseHandle(h);
            return 0;
        }
        h as usize
    })
}

/// Assign a spawned child (by PID) to our kill-on-close job.
/// Best-effort: silently no-ops when the job or the handle fails
/// (e.g. UAC-elevated children aren't ours to claim anyway).
pub fn protect(pid: u32) {
    let hjob = job();
    if hjob == 0 {
        return;
    }
    unsafe {
        let hp = OpenProcess(
            windows_sys::Win32::System::Threading::PROCESS_SET_QUOTA
                | windows_sys::Win32::System::Threading::PROCESS_TERMINATE,
            0,
            pid,
        );
        if hp.is_null() {
            return;
        }
        AssignProcessToJobObject(hjob as _, hp);
        CloseHandle(hp);
    }
}
