use serde::Serialize;
use sysinfo::System;

use super::round1;

#[derive(Debug, Clone, Serialize)]
pub struct CoreStats {
    pub name: String,
    pub usage_percent: f32,
    pub frequency_mhz: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuStats {
    pub usage_percent: f32,
    pub core_count: usize,
    /// `None` when the CPU model string is unavailable.
    pub brand: Option<String>,
    pub cores: Vec<CoreStats>,
}

/// Maps already-refreshed sysinfo state into the API shape.
pub fn collect(sys: &System) -> CpuStats {
    let cores: Vec<CoreStats> = sys
        .cpus()
        .iter()
        .map(|c| CoreStats {
            name: c.name().to_owned(),
            usage_percent: round1(c.usage()),
            frequency_mhz: c.frequency(),
        })
        .collect();
    let brand = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_owned())
        .filter(|b| !b.is_empty());
    CpuStats {
        usage_percent: round1(sys.global_cpu_usage()),
        core_count: cores.len(),
        brand,
        cores,
    }
}
