use std::time::{Duration, Instant};

/// One reading of this process's resource usage.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProcessSample {
    /// Resident set / working set.
    pub rss_bytes: u64,
    /// Memory private to the process: `PrivateUsage` on Windows, `RssAnon` on Linux.
    pub private_bytes: u64,
    /// CPU time used since the previous sample as a percentage of the whole machine's
    /// capacity (all logical CPUs), like Task Manager: 100 means every core was busy.
    pub cpu_percent: f32,
    /// OS thread count; 0 where unavailable.
    pub threads: u32,
}

/// Samples the current process. CPU usage is a delta, so the first reading covers the
/// time since [`MetricsSampler::new`].
#[derive(Debug)]
pub struct MetricsSampler {
    last_wall: Instant,
    last_cpu: Option<Duration>,
    last_percent: f32,
    logical_cpus: f64,
}

/// Readings closer together than this reuse the previous CPU figure; the delta of a
/// few milliseconds is dominated by clock granularity.
const MIN_CPU_WINDOW: Duration = Duration::from_millis(20);

impl Default for MetricsSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsSampler {
    pub fn new() -> Self {
        Self {
            last_wall: Instant::now(),
            last_cpu: platform::cpu_time(),
            last_percent: 0.0,
            logical_cpus: std::thread::available_parallelism().map_or(1, |n| n.get()) as f64,
        }
    }

    pub fn sample(&mut self) -> ProcessSample {
        let now = Instant::now();
        let window = now.duration_since(self.last_wall);
        if window >= MIN_CPU_WINDOW
            && let (Some(prev), Some(cur)) = (self.last_cpu, platform::cpu_time())
        {
            let busy = cur.saturating_sub(prev).as_secs_f64();
            self.last_percent = (busy / window.as_secs_f64() / self.logical_cpus * 100.0).clamp(0.0, 100.0) as f32;
            self.last_wall = now;
            self.last_cpu = Some(cur);
        }
        let mem = platform::memory();
        ProcessSample { rss_bytes: mem.rss, private_bytes: mem.private, cpu_percent: self.last_percent, threads: mem.threads }
    }
}

#[derive(Default)]
struct Memory {
    rss: u64,
    private: u64,
    threads: u32,
}

#[cfg(target_os = "linux")]
mod platform {
    use std::time::Duration;

    use super::Memory;

    /// `USER_HZ`; the kernel reports `/proc/<pid>/stat` times in these ticks and it is 100
    /// on every mainstream configuration.
    const CLOCK_TICKS_PER_SEC: u64 = 100;

    pub(super) fn memory() -> Memory {
        let Ok(status) = std::fs::read_to_string("/proc/self/status") else { return Memory::default() };
        let field = |name: &str| -> Option<u64> {
            let line = status.lines().find_map(|l| l.strip_prefix(name))?;
            line.trim_start_matches(':').split_whitespace().next()?.parse().ok()
        };
        let rss = field("VmRSS").unwrap_or(0) * 1024;
        Memory { rss, private: field("RssAnon").map_or(rss, |kb| kb * 1024), threads: field("Threads").map_or(0, |n| n as u32) }
    }

    pub(super) fn cpu_time() -> Option<Duration> {
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        // The command name is parenthesised and may contain spaces; fields follow the last ')'.
        // After it: state is index 0, so utime and stime (fields 14 and 15) are 11 and 12.
        let rest = &stat[stat.rfind(')')? + 1..];
        let mut fields = rest.split_whitespace();
        let utime: u64 = fields.nth(11)?.parse().ok()?;
        let stime: u64 = fields.next()?.parse().ok()?;
        Some(Duration::from_micros((utime + stime) * 1_000_000 / CLOCK_TICKS_PER_SEC))
    }
}

#[cfg(windows)]
mod platform {
    use std::mem::size_of;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetProcessTimes};

    use super::Memory;

    pub(super) fn memory() -> Memory {
        // SAFETY: a zeroed PROCESS_MEMORY_COUNTERS_EX is a valid out-parameter; `cb` tells the
        // API which struct size it may fill, and the extended struct begins with the plain one.
        let (rss, private) = unsafe {
            let mut counters: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
            counters.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            let ok = GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
                counters.cb,
            );
            if ok == 0 { (0, 0) } else { (counters.WorkingSetSize as u64, counters.PrivateUsage as u64) }
        };
        Memory { rss, private, threads: thread_count() }
    }

    pub(super) fn cpu_time() -> Option<Duration> {
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: all four out-pointers refer to live FILETIME values.
        let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user) };
        if ok == 0 {
            return None;
        }
        let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        // FILETIME counts 100 ns intervals.
        Some(Duration::from_nanos((ticks(kernel) + ticks(user)).saturating_mul(100)))
    }

    /// Counts this process's threads through a toolhelp snapshot.
    fn thread_count() -> u32 {
        // SAFETY: the snapshot handle is closed on every path; THREADENTRY32 is initialised
        // with its size as the API requires.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return 0;
            }
            let pid = GetCurrentProcessId();
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = size_of::<THREADENTRY32>() as u32;
            let mut count = 0;
            let mut more = Thread32First(snapshot, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    count += 1;
                }
                more = Thread32Next(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
            count
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use std::time::Duration;

    use super::Memory;

    pub(super) fn memory() -> Memory {
        Memory::default()
    }

    pub(super) fn cpu_time() -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sampler_matches_new() {
        let mut s = MetricsSampler::default();
        let sample = s.sample();
        assert!((0.0..=100.0).contains(&sample.cpu_percent));
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn reports_memory_and_threads() {
        let mut s = MetricsSampler::new();
        let sample = s.sample();
        assert!(sample.rss_bytes > 1024 * 1024, "{sample:?}");
        assert!(sample.private_bytes > 0, "{sample:?}");
        assert!(sample.threads >= 1, "{sample:?}");
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn rss_tracks_allocations() {
        let mut s = MetricsSampler::new();
        let before = s.sample().rss_bytes;
        let mut block = vec![0u8; 64 * 1024 * 1024];
        for chunk in block.chunks_mut(4096) {
            chunk[0] = 1;
        }
        let after = s.sample().rss_bytes;
        assert!(after >= before + 32 * 1024 * 1024, "{before} -> {after}");
        drop(std::hint::black_box(block));
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn cpu_percent_rises_under_load() {
        let mut s = MetricsSampler::new();
        let start = Instant::now();
        let mut x = 0u64;
        while start.elapsed() < Duration::from_millis(400) {
            x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407));
        }
        std::hint::black_box(x);
        let busy = s.sample();
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f32;
        // One saturated thread is 100/cores percent of the machine; allow generous slack.
        assert!(busy.cpu_percent >= 25.0 / cores, "expected load to register, got {busy:?} on {cores} cores");
        assert!(busy.cpu_percent <= 100.0);

        std::thread::sleep(Duration::from_millis(300));
        let idle = s.sample();
        assert!(idle.cpu_percent < busy.cpu_percent, "{idle:?} vs {busy:?}");
    }

    #[test]
    fn rapid_resampling_keeps_last_cpu_figure() {
        let mut s = MetricsSampler::new();
        std::thread::sleep(Duration::from_millis(30));
        let first = s.sample();
        let second = s.sample();
        assert_eq!(first.cpu_percent, second.cpu_percent);
    }
}
