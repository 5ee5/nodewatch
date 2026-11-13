use serde::Serialize;
use sysinfo::System;

use super::percent;

#[derive(Debug, Clone, Serialize)]
pub struct MemoryStats {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f32,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
}

pub fn from_bytes(
    total: u64,
    used: u64,
    available: u64,
    swap_total: u64,
    swap_used: u64,
) -> MemoryStats {
    MemoryStats {
        total_bytes: total,
        used_bytes: used,
        available_bytes: available,
        usage_percent: percent(used, total),
        swap_total_bytes: swap_total,
        swap_used_bytes: swap_used,
    }
}

pub fn collect(sys: &System) -> MemoryStats {
    from_bytes(
        sys.total_memory(),
        sys.used_memory(),
        sys.available_memory(),
        sys.total_swap(),
        sys.used_swap(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_percent() {
        let m = from_bytes(1000, 250, 750, 0, 0);
        assert_eq!(m.usage_percent, 25.0);
    }

    #[test]
    fn zero_total_does_not_divide_by_zero() {
        let m = from_bytes(0, 0, 0, 0, 0);
        assert_eq!(m.usage_percent, 0.0);
    }
}
