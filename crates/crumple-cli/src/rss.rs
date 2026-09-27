//! Peak resident set size of this process, in bytes.

#[cfg(windows)]
pub(crate) fn peak_rss_bytes() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: the counters struct is plain data, correctly sized via `cb`, and
    // the pseudo-handle from GetCurrentProcess needs no closing.
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = cb;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, cb) == 0 {
            0
        } else {
            c.PeakWorkingSetSize as u64
        }
    }
}

#[cfg(unix)]
pub(crate) fn peak_rss_bytes() -> u64 {
    // SAFETY: getrusage only writes into the zeroed struct we pass.
    let max_rss = unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut u) != 0 {
            return 0;
        }
        u.ru_maxrss
    };
    let v = max_rss.max(0) as u64;
    if cfg!(target_os = "macos") {
        v
    } else {
        v * 1024
    }
}

#[cfg(not(any(windows, unix)))]
pub(crate) fn peak_rss_bytes() -> u64 {
    0
}
