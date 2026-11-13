//! Metric collection.
//!
//! A single background task owns the sysinfo state and publishes a `Snapshot`.
//! HTTP handlers only read the latest snapshot, so request load can never
//! increase collection work, and CPU percentages (which need two samples
//! spaced apart) are never computed inside a handler.

pub mod cpu;
pub mod disks;
pub mod memory;
pub mod system;

use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};
use tokio::time::{self, MissedTickBehavior};
use tracing::{error, info};

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// Unix time in milliseconds when this sample finished.
    pub collected_at_ms: u64,
    /// How often the sampler refreshes; lets clients judge staleness.
    pub interval_secs: u64,
    pub system: system::SystemInfo,
    pub cpu: cpu::CpuStats,
    pub memory: memory::MemoryStats,
    pub disks: Vec<disks::DiskStats>,
}

/// Latest snapshot, `None` until the first sample completes.
#[derive(Clone, Default)]
pub struct SharedSnapshot(Arc<RwLock<Option<Arc<Snapshot>>>>);

impl SharedSnapshot {
    pub fn get(&self) -> Option<Arc<Snapshot>> {
        // A poisoned lock only means a writer panicked mid-store of an Arc; the data is still valid.
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set(&self, snapshot: Snapshot) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(snapshot));
    }
}

pub fn percent(used: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    round1((used as f64 / total as f64 * 100.0) as f32)
}

pub fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

/// Owns the stateful sysinfo handles. Not `Sync`-shared: it moves into
/// `spawn_blocking` for each sample and back out.
pub struct Collector {
    sys: System,
    disks: Disks,
    interval: Duration,
}

impl Collector {
    pub fn new(interval: Duration) -> Result<Self, sysinfo::Error> {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        )?;
        let disks = Disks::new_with_refreshed_list()?;
        Ok(Self {
            sys,
            disks,
            interval,
        })
    }

    /// Reads fresh values. Blocking: touches /proc, /sys and statvfs on every mount.
    pub fn sample(&mut self) -> Snapshot {
        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();
        self.disks.refresh(true);
        Snapshot {
            collected_at_ms: now_ms(),
            interval_secs: self.interval.as_secs(),
            system: system::collect(),
            cpu: cpu::collect(&self.sys),
            memory: memory::collect(&self.sys),
            disks: disks::collect(&self.disks),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Starts the sampler. Returns once the first sample is available so the
/// server never serves an all-zero CPU reading.
pub async fn spawn(interval: Duration) -> Result<SharedSnapshot, sysinfo::Error> {
    let shared = SharedSnapshot::default();
    let mut collector = Collector::new(interval)?;

    // Warm-up: CPU usage is the delta between two reads.
    collector.sys.refresh_cpu_all();
    time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    let (mut collector, first) = run_blocking(collector).await.map_err(|e| {
        error!("initial sample failed: {e}");
        sysinfo::Error::from(std::io::Error::other("initial sample failed"))
    })?;
    shared.set(first);
    info!("first metrics sample ready");

    let publish = shared.clone();
    tokio::spawn(async move {
        let mut ticker = time::interval(interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        ticker.tick().await; // the first tick fires immediately; we just sampled
        loop {
            ticker.tick().await;
            match run_blocking(collector).await {
                Ok((c, snap)) => {
                    collector = c;
                    publish.set(snap);
                }
                Err(e) => {
                    // The collector was lost with the panicked task; rebuild it.
                    error!("sampling task failed: {e}");
                    match Collector::new(interval) {
                        Ok(c) => collector = c,
                        Err(e) => {
                            error!("cannot rebuild collector, sampler stopped: {e}");
                            return;
                        }
                    }
                }
            }
        }
    });
    Ok(shared)
}

async fn run_blocking(mut c: Collector) -> Result<(Collector, Snapshot), tokio::task::JoinError> {
    tokio::task::spawn_blocking(move || {
        let snap = c.sample();
        (c, snap)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_handles_zero_and_rounds() {
        assert_eq!(percent(1, 0), 0.0);
        assert_eq!(percent(1, 3), 33.3);
    }

    #[test]
    fn real_host_sample_is_sane() {
        let snap = Collector::new(Duration::from_secs(2))
            .expect("collector")
            .sample();
        assert!(snap.memory.total_bytes > 0);
        assert!(snap.cpu.core_count > 0);
        assert_eq!(snap.cpu.core_count, snap.cpu.cores.len());
        assert!(snap.memory.used_bytes <= snap.memory.total_bytes);
    }
}
