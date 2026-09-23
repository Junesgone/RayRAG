//! Host resource detection and the runtime parameters derived from it.
//!
//! The deployment sizes itself from the machine it actually runs on instead of
//! shipping a fixed guess: an NAS with four cores and 8 GB must not ingest four
//! documents at once, and a 128 GB workstation with sixteen cores can afford more
//! than the conservative default. Everything stays overridable — an explicit
//! `RAYRAG_MAX_CONCURRENT_TASKS` (or any other documented knob) always wins, so the
//! automatic value is a starting point, not a policy.
//!
//! Only `/proc/meminfo` is read (Linux); anywhere else the memory figure is simply
//! unknown and the CPU count alone decides.

use std::path::Path;

/// Bytes this process assumes one in-flight document costs while it is parsed,
/// chunked and embedded (file bytes plus parsed text plus vectors plus parser
/// scratch space). Deliberately a worst-case figure for a large document: sizing the
/// batch from a small document would be exactly the mistake that lets ingestion eat
/// the machine.
pub const BYTES_PER_DOCUMENT_TASK: u64 = 512 << 20;

/// Upper bound for the derived worker count: parallelism past this point buys
/// little and multiplies the memory a single batch can hold.
pub const MAX_DOCUMENT_TASKS: usize = 8;

/// What the host looks like right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostResources {
    /// Logical CPUs (`available_parallelism`), at least 1.
    pub cpus: usize,
    /// `MemAvailable` from `/proc/meminfo`, when the host exposes it.
    pub available_memory_bytes: Option<u64>,
}

impl HostResources {
    /// Detect the current host.
    pub fn detect() -> Self {
        Self {
            cpus: std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(1),
            available_memory_bytes: read_mem_available(Path::new("/proc/meminfo")),
        }
    }

    /// How many documents may be parsed concurrently.
    ///
    /// Half the CPUs (the other half stays available for HTTP and search) limited by
    /// how many `BYTES_PER_DOCUMENT_TASK` slices the *available* memory can hold,
    /// and never more than [`MAX_DOCUMENT_TASKS`]. Memory is judged against 3/4 of
    /// what is free so the process cannot eat the whole machine on its own.
    pub fn document_task_limit(&self) -> usize {
        let by_cpu = (self.cpus / 2).max(1);
        let by_memory = match self.available_memory_bytes {
            Some(bytes) => {
                let budget = bytes / 4 * 3;
                ((budget / BYTES_PER_DOCUMENT_TASK).max(1)) as usize
            }
            None => MAX_DOCUMENT_TASKS,
        };
        by_cpu.min(by_memory).clamp(1, MAX_DOCUMENT_TASKS)
    }

    /// One-line summary for the startup log and `/api/v1/system/status`.
    pub fn summary(&self) -> String {
        match self.available_memory_bytes {
            Some(bytes) => format!(
                "{} cpus, {:.1} GiB available",
                self.cpus,
                bytes as f64 / (1024.0 * 1024.0 * 1024.0)
            ),
            None => format!("{} cpus, available memory unknown", self.cpus),
        }
    }
}

/// Parse `MemAvailable` (kB) out of `/proc/meminfo`-style content.
pub fn parse_mem_available(content: &str) -> Option<u64> {
    for line in content.lines() {
        let Some(rest) = line.strip_prefix("MemAvailable:") else {
            continue;
        };
        let value = rest
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u64>().ok())?;
        // `/proc/meminfo` reports kB.
        return Some(value.saturating_mul(1024));
    }
    None
}

fn read_mem_available(path: &Path) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    parse_mem_available(&content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_available_parses_kilobytes_and_ignores_other_lines() {
        let sample = "MemTotal:       131602040 kB\nMemFree:         1000000 kB\nMemAvailable:   115793596 kB\nBuffers:               0 kB\n";
        assert_eq!(parse_mem_available(sample), Some(115_793_596 * 1024));
        assert_eq!(parse_mem_available("MemTotal: 1 kB\n"), None);
        assert_eq!(parse_mem_available(""), None);
    }

    #[test]
    fn document_tasks_scale_with_cpus_and_memory_but_stay_bounded() {
        // Small NAS: few cores -> one document at a time, however much memory is free.
        let small = HostResources {
            cpus: 2,
            available_memory_bytes: Some(2 << 30),
        };
        assert_eq!(small.document_task_limit(), 1);

        // Four cores but only 2 GiB free: two documents, and no more.
        let modest = HostResources {
            cpus: 4,
            available_memory_bytes: Some(2 << 30),
        };
        assert_eq!(modest.document_task_limit(), 2);

        // A roomy workstation: capped by MAX_DOCUMENT_TASKS, never more.
        let large = HostResources {
            cpus: 64,
            available_memory_bytes: Some(256 << 30),
        };
        assert_eq!(large.document_task_limit(), MAX_DOCUMENT_TASKS);

        // Something in between: eight cores and 12 GiB free allows four workers.
        let middle = HostResources {
            cpus: 8,
            available_memory_bytes: Some(12 << 30),
        };
        assert_eq!(middle.document_task_limit(), 4);

        // Unknown memory falls back to the CPU-derived value, still capped.
        let unknown = HostResources {
            cpus: 16,
            available_memory_bytes: None,
        };
        assert_eq!(unknown.document_task_limit(), MAX_DOCUMENT_TASKS);
        assert!(unknown.summary().contains("unknown"));

        // Never zero, even on a single-core host with almost no memory.
        let tiny = HostResources {
            cpus: 1,
            available_memory_bytes: Some(64 << 20),
        };
        assert_eq!(tiny.document_task_limit(), 1);
    }

    #[test]
    fn detection_reports_this_host() {
        let resources = HostResources::detect();
        assert!(resources.cpus >= 1);
        // Linux exposes MemAvailable; the summary is always printable.
        assert!(!resources.summary().is_empty());
        assert!((1..=MAX_DOCUMENT_TASKS).contains(&resources.document_task_limit()));
    }
}
