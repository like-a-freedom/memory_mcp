//! Process-memory readers that no external collector can supply.
//!
//! Two signals live only inside this process and are invisible to cAdvisor:
//! the glibc allocator's own accounting (`mallinfo2`) and the `[heap]`
//! mapping extent from `/proc/self/maps`. They are the attribution the
//! footprint investigation needs — RSS says *how much*, these say *which
//! arena* — and they belong to `platform` because they are a technical
//! read of the host, carrying no business orchestration.
//!
//! Every field is optional and every read is total: off Linux/glibc the
//! answer is `None`, and a `None` is never recorded as `0`. A fabricated
//! zero is a worse signal than a missing one — it reads as "the allocator
//! holds nothing", which is exactly the wrong conclusion to reach from a
//! platform that has no allocator telemetry at all.

/// glibc `malloc` accounting plus the `[heap]` mapping extent.
///
/// Every field is optional: off Linux/glibc there is no `mallinfo2` call and
/// no heap extent, so every field is `None`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AllocSnapshot {
    /// Total bytes in the main arena (`mallinfo2.arena`).
    pub(crate) arena_bytes: Option<u64>,
    /// Bytes in mmap'd blocks at or above the mmap threshold (`hblkhd`).
    pub(crate) mmap_bytes: Option<u64>,
    /// Bytes in in-use blocks (`uordblks`) — the live allocations.
    pub(crate) in_use_bytes: Option<u64>,
    /// Bytes in free blocks (`fordblks`) — retained, not returned to the OS.
    pub(crate) free_bytes: Option<u64>,
    /// Bytes at the top of the heap releasable by `malloc_trim` (`keepcost`).
    pub(crate) keepcost_bytes: Option<u64>,
    /// Width of the `[heap]` mapping in `/proc/self/maps`, in bytes.
    pub(crate) heap_bytes: Option<u64>,
}

/// Read glibc's allocator accounting and the `[heap]` extent.
///
/// Read-only and total: it never panics and never fabricates a value. When a
/// signal is unavailable the corresponding field is `None`.
pub(crate) fn read_alloc() -> AllocSnapshot {
    let mut snapshot = AllocSnapshot {
        heap_bytes: read_proc_self_maps().and_then(|maps| parse_heap_bytes(&maps)),
        ..AllocSnapshot::default()
    };
    fill_from_mallinfo2(&mut snapshot);
    snapshot
}

/// Fill the `mallinfo2` fields on Linux/glibc.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn fill_from_mallinfo2(snapshot: &mut AllocSnapshot) {
    // SAFETY: `mallinfo2` takes no arguments and returns a plain struct of
    // `size_t` values by value. It is async-signal-safe, never allocates and
    // cannot fail, so its return value is always fully initialised.
    let info = unsafe { mallinfo2() };
    snapshot.arena_bytes = Some(info.arena as u64);
    snapshot.mmap_bytes = Some(info.hblkhd as u64);
    snapshot.in_use_bytes = Some(info.uordblks as u64);
    snapshot.free_bytes = Some(info.fordblks as u64);
    snapshot.keepcost_bytes = Some(info.keepcost as u64);
}

/// Off Linux/glibc there is no `mallinfo2`: every field stays `None`.
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn fill_from_mallinfo2(_snapshot: &mut AllocSnapshot) {}

/// `/proc/self/maps` on Linux/glibc, or `None` anywhere else.
///
/// "Not glibc" and "no file" collapse to the same answer on purpose: the heap
/// extent only means something alongside glibc's `mallinfo2`, and reporting it
/// from an allocator we do not otherwise measure would mix two vocabularies in
/// one family.
fn read_proc_self_maps() -> Option<String> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        std::fs::read_to_string("/proc/self/maps").ok()
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        None
    }
}

/// Bytes spanned by the `[heap]` mapping, or `None` when the file has none.
///
/// The width of the address range, not the resident pages: `/proc` maps are
/// already denominated in bytes, and this is the extent the allocator may grow
/// into — the number that read 1.14 GB on the host. A file with no `[heap]`
/// line is `None`, never `0`: the two mean different things.
fn parse_heap_bytes(maps: &str) -> Option<u64> {
    for line in maps.lines() {
        if !line.ends_with("[heap]") {
            continue;
        }
        let range = line.split_whitespace().next()?;
        let (start, end) = range.split_once('-')?;
        let start = u64::from_str_radix(start, 16).ok()?;
        let end = u64::from_str_radix(end, 16).ok()?;
        return end.checked_sub(start);
    }
    None
}

/// The process's own memory totals, from `/proc/self/status`.
///
/// Gated to the build that has a caller for them — the startup marker in
/// `http::runtime::bootstrap` — because the allocator gauges are not the only
/// process-memory read: these are logs, never a family, and a `pub(crate)` item
/// with no caller in a build is dead code under the zero-warning gate. The
/// parser test below keeps them live in any test build.
#[cfg(any(test, feature = "streamable-http"))]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessTotals {
    /// Resident set size (`VmRSS`).
    pub(crate) rss_bytes: Option<u64>,
    /// Swapped-out bytes (`VmSwap`).
    pub(crate) swap_bytes: Option<u64>,
    /// Peak resident set size (`VmHWM`) — the boot-time high-water mark.
    pub(crate) hwm_bytes: Option<u64>,
}

/// Read RSS, swap and the peak from `/proc/self/status`; all `None` off Linux.
#[cfg(any(test, feature = "streamable-http"))]
pub(crate) fn read_process_totals() -> ProcessTotals {
    match read_proc_self_status() {
        Some(status) => parse_proc_status(&status),
        None => ProcessTotals::default(),
    }
}

/// `/proc/self/status` on Linux, or `None` anywhere else.
#[cfg(any(test, feature = "streamable-http"))]
fn read_proc_self_status() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/status").ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Parse `VmRSS`, `VmSwap` and `VmHWM`. Each is reported in kB; return bytes.
#[cfg(any(test, feature = "streamable-http"))]
fn parse_proc_status(status: &str) -> ProcessTotals {
    ProcessTotals {
        rss_bytes: status_kib(status, "VmRSS:"),
        swap_bytes: status_kib(status, "VmSwap:"),
        hwm_bytes: status_kib(status, "VmHWM:"),
    }
}

/// The first numeric value after `key`, converted from kB to bytes.
#[cfg(any(test, feature = "streamable-http"))]
fn status_kib(status: &str, key: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|kib| kib.saturating_mul(1024))
}

/// glibc `struct mallinfo2`, returned by `mallinfo2(3)`.
///
/// The field order is fixed by glibc's ABI. Only the fields this module reads
/// are named; the rest are declared so the returned struct is exactly the
/// `size_t`-wide shape glibc writes.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[repr(C)]
struct Mallinfo2 {
    arena: usize,
    ordblks: usize,
    smblks: usize,
    hblks: usize,
    hblkhd: usize,
    usmblks: usize,
    fsmblks: usize,
    uordblks: usize,
    fordblks: usize,
    keepcost: usize,
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    /// glibc's byte-denominated `malloc` statistics.
    fn mallinfo2() -> Mallinfo2;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_heap_extent_in_maps() {
        // A well-formed maps range whose width is 0x100 pages: the parser reads
        // `end - start`, and the fixture's extent is what the assertion pins.
        let maps = "7f0000000000-7f0000100000 rw-p 00000000 00:00 0 [heap]\n";
        assert_eq!(parse_heap_bytes(maps), Some(0x100 * 4096));
    }

    #[test]
    fn maps_without_heap_is_none_not_zero() {
        assert_eq!(
            parse_heap_bytes(
                "7f0100000000-7f0100010000 r-xp 00000000 00:00 0 /usr/lib/libc.so.6\n"
            ),
            None
        );
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn read_alloc_reports_the_glibc_arena() {
        let alloc = read_alloc();
        assert!(
            alloc.arena_bytes.is_some(),
            "glibc mallinfo2 is available on this host, so the arena is measured"
        );
        assert!(alloc.in_use_bytes.is_some());
    }

    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    #[test]
    fn read_alloc_is_all_none_off_glibc() {
        assert_eq!(
            read_alloc(),
            AllocSnapshot::default(),
            "off glibc every field is None, never zero"
        );
    }

    #[test]
    fn parses_rss_swap_and_peak_from_proc_status() {
        let status = "Name:\tmemory_mcp\nVmPeak:\t  500000 kB\nVmRSS:\t  1234 kB\n\
                      VmSwap:\t    56 kB\nVmHWM:\t  7890 kB\n";
        let totals = parse_proc_status(status);
        assert_eq!(totals.rss_bytes, Some(1234 * 1024));
        assert_eq!(totals.swap_bytes, Some(56 * 1024));
        assert_eq!(totals.hwm_bytes, Some(7890 * 1024));
    }

    #[test]
    fn missing_proc_status_keys_are_none_not_zero() {
        let totals = parse_proc_status("Name:\tmemory_mcp\n");
        assert_eq!(
            totals,
            ProcessTotals::default(),
            "a key the file does not carry is None, never zero"
        );
    }

    /// Where `/proc/self/status` exists, the live reader reports this process.
    ///
    /// The parser test above feeds fixtures; this is the one that proves the
    /// reader is wired to the real file and returns a number, not just that a
    /// string can be parsed.
    #[cfg(target_os = "linux")]
    #[test]
    fn read_process_totals_reads_the_live_process_on_linux() {
        let totals = read_process_totals();
        assert!(
            totals.rss_bytes.is_some(),
            "/proc/self/status reports VmRSS on Linux"
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn read_process_totals_is_all_none_off_linux() {
        assert_eq!(
            read_process_totals(),
            ProcessTotals::default(),
            "with no /proc every field is None, never zero"
        );
    }
}
