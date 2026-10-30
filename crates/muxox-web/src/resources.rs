//! CPU, memory, and GPU for a service process and its descendants.
//!
//! The pid muxox records is the shell it spawned. The workload (`bun`, `node`,
//! and so on) is a child of that shell, so a sample sums the whole tree.
//! A process that double-forks and reparents to init is not included.
//! GPU work inside WindowServer or a driver daemon is not included either.

use serde::Serialize;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// CPU, resident memory, and GPU for one service.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Resources {
    /// Percent of one core since the previous sample. Above 100 on multi-core.
    pub cpu_percent: f32,
    /// Resident set size in bytes, summed across the process tree.
    pub memory_bytes: u64,
    /// Share of all GPUs since the previous sample. `None` when unavailable.
    pub gpu_percent: Option<f32>,
    /// GPU memory in bytes, summed across the process tree. Not strictly VRAM.
    /// `None` when unavailable. macOS never reports this.
    pub gpu_memory_bytes: Option<u64>,
}

struct ProcSample {
    pid: u32,
    parent: Option<u32>,
    cpu_percent: f32,
    memory_bytes: u64,
    gpu_percent: Option<f32>,
    gpu_memory_bytes: Option<u64>,
    is_thread: bool,
}

/// Sum CPU, RSS, and GPU for `root` and every descendant.
///
/// Returns `None` when `root` is not in `procs`. Threads are walked so their
/// children still count, and their own CPU, memory, and GPU are skipped. Linux
/// reports a thread's RSS as the whole process, so counting it would multiply
/// memory by the thread count.
///
/// A GPU field stays `None` when every counted process lacks that reading.
/// Once any process reports it, missing siblings count as zero.
fn sum_tree(procs: &[ProcSample], root: u32) -> Option<Resources> {
    let mut by_pid = std::collections::HashMap::<u32, &ProcSample>::new();
    let mut children = std::collections::HashMap::<u32, Vec<u32>>::new();
    for proc in procs {
        by_pid.insert(proc.pid, proc);
        if let Some(parent) = proc.parent {
            children.entry(parent).or_default().push(proc.pid);
        }
    }
    if !by_pid.contains_key(&root) {
        return None;
    }

    let mut cpu_percent = 0.0f32;
    let mut memory_bytes = 0u64;
    let mut gpu_percent = None;
    let mut gpu_memory_bytes = None;
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(proc) = by_pid.get(&pid) {
            let count_self = pid == root || !proc.is_thread;
            if count_self {
                if proc.cpu_percent.is_finite() && proc.cpu_percent > 0.0 {
                    cpu_percent += proc.cpu_percent;
                }
                memory_bytes = memory_bytes.saturating_add(proc.memory_bytes);
                add_gpu_percent(&mut gpu_percent, proc.gpu_percent);
                add_gpu_memory(&mut gpu_memory_bytes, proc.gpu_memory_bytes);
            }
        }
        if let Some(kids) = children.get(&pid) {
            stack.extend(kids.iter().copied());
        }
    }
    Some(Resources {
        cpu_percent,
        memory_bytes,
        gpu_percent,
        gpu_memory_bytes,
    })
}

fn add_gpu_percent(total: &mut Option<f32>, sample: Option<f32>) {
    let Some(sample) = sample else {
        return;
    };
    let sample = if sample.is_finite() && sample > 0.0 {
        sample
    } else {
        0.0
    };
    *total = Some(total.unwrap_or(0.0) + sample);
}

fn add_gpu_memory(total: &mut Option<u64>, sample: Option<u64>) {
    let Some(sample) = sample else {
        return;
    };
    *total = Some(total.unwrap_or(0).saturating_add(sample));
}

pub struct Sampler {
    sys: System,
    primed: bool,
}

impl Sampler {
    pub fn new() -> Result<Self, sysinfo::Error> {
        Ok(Self {
            sys: System::new()?,
            primed: false,
        })
    }

    /// Sample each root pid. The first call only primes CPU counters and
    /// returns `None` for every root. Later calls return `None` when that
    /// root is missing or was not requested.
    pub fn sample(&mut self, roots: &[Option<u32>]) -> Vec<Option<Resources>> {
        let kind = ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_gpu_usage()
            .with_gpu_memory()
            .without_tasks();
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        if !self.primed {
            self.primed = true;
            return vec![None; roots.len()];
        }

        let procs: Vec<ProcSample> = self
            .sys
            .processes()
            .iter()
            .map(|(pid, proc)| ProcSample {
                pid: pid.as_u32(),
                parent: proc.parent().map(|parent| parent.as_u32()),
                cpu_percent: proc.cpu_usage(),
                memory_bytes: proc.memory(),
                gpu_percent: proc.gpu_usage(),
                gpu_memory_bytes: proc.gpu_memory(),
                is_thread: proc.thread_kind().is_some(),
            })
            .collect();
        roots
            .iter()
            .map(|root| root.and_then(|pid| sum_tree(&procs, pid)))
            .collect()
    }
}

/// Same rules as `formatCpu` / `formatBytes` in `web_script.js`.
pub(crate) fn format_cpu(percent: f32) -> String {
    let percent = if percent.is_finite() && percent > 0.0 {
        percent
    } else {
        0.0
    };
    format!("{percent:.1}%")
}

/// Header and `inspect` text. GPU segments appear only when that reading exists.
pub(crate) fn format_resources(sample: Resources) -> String {
    let mut text = format!(
        "{} · {}",
        format_cpu(sample.cpu_percent),
        format_bytes(sample.memory_bytes)
    );
    match (sample.gpu_percent, sample.gpu_memory_bytes) {
        (None, None) => {}
        (Some(percent), None) => {
            text.push_str(" · GPU ");
            text.push_str(&format_cpu(percent));
        }
        (None, Some(bytes)) => {
            text.push_str(" · GPU ");
            text.push_str(&format_bytes(bytes));
        }
        (Some(percent), Some(bytes)) => {
            text.push_str(" · GPU ");
            text.push_str(&format_cpu(percent));
            text.push_str(" · ");
            text.push_str(&format_bytes(bytes));
        }
    }
    text
}

/// Binary units. Bytes are an integer. Larger units keep one decimal.
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", value.round() as u64)
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, parent: Option<u32>, cpu: f32, mem: u64, is_thread: bool) -> ProcSample {
        ProcSample {
            pid,
            parent,
            cpu_percent: cpu,
            memory_bytes: mem,
            gpu_percent: None,
            gpu_memory_bytes: None,
            is_thread,
        }
    }

    fn with_gpu(mut proc: ProcSample, percent: Option<f32>, memory: Option<u64>) -> ProcSample {
        proc.gpu_percent = percent;
        proc.gpu_memory_bytes = memory;
        proc
    }

    #[test]
    fn sum_tree_includes_descendants_only() {
        let procs = vec![
            proc(1, None, 1.0, 100, false),
            proc(2, Some(1), 2.5, 200, false),
            proc(3, Some(2), 0.5, 50, false),
            proc(9, Some(8), 10.0, 999, false),
        ];
        let got = sum_tree(&procs, 1).unwrap();
        assert_eq!(got.cpu_percent, 4.0);
        assert_eq!(got.memory_bytes, 350);
        assert_eq!(got.gpu_percent, None);
        assert_eq!(got.gpu_memory_bytes, None);
        assert!(sum_tree(&procs, 42).is_none());
    }

    #[test]
    fn sum_tree_keeps_gpu_absent_until_any_process_reports_it() {
        let procs = vec![
            with_gpu(proc(1, None, 1.0, 100, false), Some(2.0), None),
            with_gpu(proc(2, Some(1), 1.0, 50, false), None, Some(4096)),
            with_gpu(proc(3, Some(1), 1.0, 10, true), Some(80.0), Some(999)),
            with_gpu(proc(4, Some(3), 1.0, 5, false), Some(f32::NAN), Some(8)),
        ];
        let got = sum_tree(&procs, 1).unwrap();
        assert_eq!(got.gpu_percent, Some(2.0));
        assert_eq!(got.gpu_memory_bytes, Some(4104));
    }

    #[test]
    fn sum_tree_skips_thread_memory_and_keeps_grandchildren() {
        let procs = vec![
            proc(1, None, 1.0, 100, false),
            proc(2, Some(1), 50.0, 100, true),
            proc(3, Some(2), 3.0, 40, false),
        ];
        let got = sum_tree(&procs, 1).unwrap();
        assert_eq!(got.cpu_percent, 4.0);
        assert_eq!(got.memory_bytes, 140);
    }

    #[test]
    fn sum_tree_ignores_negative_and_non_finite_cpu() {
        let procs = vec![
            proc(1, None, f32::NAN, 10, false),
            proc(2, Some(1), -5.0, 5, false),
        ];
        let got = sum_tree(&procs, 1).unwrap();
        assert_eq!(got.cpu_percent, 0.0);
        assert_eq!(got.memory_bytes, 15);
    }

    #[test]
    fn formatters_match_the_header_rules() {
        assert_eq!(format_cpu(4.24), "4.2%");
        assert_eq!(format_cpu(0.0), "0.0%");
        assert_eq!(format_cpu(-1.0), "0.0%");
        assert_eq!(format_cpu(f32::NAN), "0.0%");
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(48 * 1024 * 1024), "48.0 MB");
        let base = Resources {
            cpu_percent: 1.5,
            memory_bytes: 2048,
            gpu_percent: None,
            gpu_memory_bytes: None,
        };
        assert_eq!(format_resources(base), "1.5% · 2.0 KB");
        assert_eq!(
            format_resources(Resources {
                gpu_percent: Some(12.0),
                ..base
            }),
            "1.5% · 2.0 KB · GPU 12.0%"
        );
        assert_eq!(
            format_resources(Resources {
                gpu_percent: Some(12.04),
                gpu_memory_bytes: Some(256 * 1024 * 1024),
                ..base
            }),
            "1.5% · 2.0 KB · GPU 12.0% · 256.0 MB"
        );
        assert_eq!(
            format_resources(Resources {
                gpu_memory_bytes: Some(512),
                ..base
            }),
            "1.5% · 2.0 KB · GPU 512 B"
        );
    }

    #[test]
    fn samples_sleep_resident_memory() {
        let child = match std::process::Command::new("sleep").arg("30").spawn() {
            Ok(child) => child,
            Err(_) => return,
        };
        struct Kill(std::process::Child);
        impl Drop for Kill {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let child = Kill(child);
        let pid = child.0.id();

        let mut sampler = Sampler::new().expect("sysinfo");
        let roots = vec![Some(pid)];
        assert!(sampler.sample(&roots)[0].is_none());
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let sample = sampler.sample(&roots).into_iter().next().unwrap();
        let Some(sample) = sample else {
            panic!("sleep pid {pid} was not visible to sysinfo");
        };
        assert!(
            sample.memory_bytes > 0,
            "expected rss for pid {pid}, got {}",
            sample.memory_bytes
        );
    }
}
