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

/// One GPU as reported by `nvidia-smi`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuInfo {
    pub name: String,
    /// `memory.total` in bytes, when `nvidia-smi` reported it.
    pub memory_bytes: Option<u64>,
}

/// Parse `nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits`
/// output. Lines without a name are ignored; the memory column accepts the
/// plain MiB figure (nounits) and a trailing `MiB`.
pub fn parse_nvidia_smi_csv(content: &str) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    for line in content.lines() {
        let mut columns = line.split(',').map(str::trim);
        let Some(name) = columns.next().filter(|name| !name.is_empty()) else {
            continue;
        };
        let memory_bytes = columns
            .next()
            .map(|value| value.strip_suffix("MiB").unwrap_or(value))
            .map(str::trim)
            .and_then(|value| value.parse::<u64>().ok())
            .map(|mib| mib.saturating_mul(1024 * 1024));
        gpus.push(GpuInfo {
            name: name.to_string(),
            memory_bytes,
        });
    }
    gpus
}

/// Detect NVIDIA GPUs via `nvidia-smi` (best effort: missing driver or binary
/// yields an empty list).
pub fn detect_gpus() -> Vec<GpuInfo> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            parse_nvidia_smi_csv(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

/// One-line GPU summary for logs (`""` when no GPU is visible).
pub fn gpu_summary(gpus: &[GpuInfo]) -> String {
    if gpus.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = gpus
        .iter()
        .map(|gpu| match gpu.memory_bytes {
            Some(bytes) => format!("{} ({:.0} MiB)", gpu.name, bytes as f64 / (1024.0 * 1024.0)),
            None => gpu.name.clone(),
        })
        .collect();
    format!("{} × [{}]", gpus.len(), parts.join("; "))
}


