use std::collections::HashSet;

use serde::Serialize;
use sysinfo::Disks;

use super::percent;

/// Pseudo / container-layer filesystems that are noise on a dashboard.
const IGNORED_FS: &[&str] = &[
    "overlay", "squashfs", "tmpfs", "devtmpfs", "ramfs", "proc", "sysfs", "cgroup", "cgroup2",
    "erofs",
];

#[derive(Debug, Clone, Serialize)]
pub struct DiskStats {
    pub mount_point: String,
    pub name: String,
    pub file_system: String,
    pub total_bytes: u64,
    /// `total - available`; includes blocks reserved for root, like `df`'s "Used".
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f32,
    pub read_only: bool,
    pub removable: bool,
}

fn is_relevant(name: &str, file_system: &str, total: u64) -> bool {
    total > 0 && !name.starts_with("/dev/loop") && !IGNORED_FS.contains(&file_system)
}

fn build(
    mount_point: String,
    name: String,
    file_system: String,
    total: u64,
    available: u64,
    read_only: bool,
    removable: bool,
) -> DiskStats {
    let used = total.saturating_sub(available);
    DiskStats {
        mount_point,
        name,
        file_system,
        total_bytes: total,
        used_bytes: used,
        available_bytes: available,
        usage_percent: percent(used, total),
        read_only,
        removable,
    }
}

/// Keeps the first entry per mount point and sorts by mount point.
fn dedupe_sorted(mut disks: Vec<DiskStats>) -> Vec<DiskStats> {
    let mut seen = HashSet::new();
    disks.retain(|d| seen.insert(d.mount_point.clone()));
    disks.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    disks
}

pub fn collect(disks: &Disks) -> Vec<DiskStats> {
    let all = disks
        .iter()
        .filter_map(|d| {
            let name = d.name().to_string_lossy().into_owned();
            let fs = d.file_system().to_string_lossy().into_owned();
            if !is_relevant(&name, &fs, d.total_space()) {
                return None;
            }
            Some(build(
                d.mount_point().to_string_lossy().into_owned(),
                name,
                fs,
                d.total_space(),
                d.available_space(),
                d.is_read_only(),
                d.is_removable(),
            ))
        })
        .collect();
    dedupe_sorted(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(mount: &str, total: u64, avail: u64) -> DiskStats {
        build(
            mount.into(),
            "/dev/sda1".into(),
            "ext4".into(),
            total,
            avail,
            false,
            false,
        )
    }

    #[test]
    fn filters_pseudo_and_empty() {
        assert!(is_relevant("/dev/sda1", "ext4", 10));
        assert!(!is_relevant("/dev/sda1", "ext4", 0));
        assert!(!is_relevant("/dev/loop3", "ext4", 10));
        assert!(!is_relevant("overlay", "overlay", 10));
        assert!(!is_relevant("none", "squashfs", 10));
    }

    #[test]
    fn dedupes_by_mount_point_and_sorts() {
        let out = dedupe_sorted(vec![
            disk("/var", 10, 5),
            disk("/", 10, 5),
            disk("/var", 99, 1),
        ]);
        let mounts: Vec<_> = out.iter().map(|d| d.mount_point.as_str()).collect();
        assert_eq!(mounts, ["/", "/var"]);
        assert_eq!(out[1].total_bytes, 10, "first entry wins");
    }

    #[test]
    fn used_never_underflows() {
        let d = disk("/", 10, 20);
        assert_eq!(d.used_bytes, 0);
        assert_eq!(disk("/", 100, 25).usage_percent, 75.0);
    }
}
