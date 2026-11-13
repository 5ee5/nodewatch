use serde::Serialize;
use sysinfo::System;

/// Host identity. Each field is `None` if the OS refuses to tell us, so one
/// missing metric never fails the whole response.
#[derive(Debug, Clone, Serialize)]
pub struct SystemInfo {
    pub hostname: Option<String>,
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub uptime_secs: Option<u64>,
    pub cpu_arch: Option<String>,
}

/// "Gentoo 2.18" style label; `long_os_version()` is unreadable on some distros.
fn os_label(name: Option<String>, version: Option<String>) -> Option<String> {
    let parts: Vec<String> = [name, version]
        .into_iter()
        .flatten()
        .map(|s| s.trim().trim_matches(['\'', '"']).trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

pub fn collect() -> SystemInfo {
    SystemInfo {
        hostname: System::host_name().ok(),
        os: os_label(System::name().ok(), System::os_version().ok()),
        kernel: System::kernel_version().ok(),
        uptime_secs: System::uptime().ok(),
        cpu_arch: Some(System::cpu_arch()).filter(|a| !a.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::os_label;

    #[test]
    fn joins_available_parts() {
        let s = |v: &str| Some(v.to_owned());
        assert_eq!(os_label(s("Gentoo"), s("2.18")), s("Gentoo 2.18"));
        assert_eq!(os_label(s("Gentoo"), None), s("Gentoo"));
        assert_eq!(os_label(None, s(" ")), None);
        // sysinfo does not unquote os-release values on some distros.
        assert_eq!(os_label(s("'Gentoo'"), s("\"2.18\"")), s("Gentoo 2.18"));
    }
}
