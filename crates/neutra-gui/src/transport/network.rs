#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::process::Command;

pub(super) fn discover_network_hosts() -> Vec<(String, String)> {
    #[cfg(target_os = "linux")]
    {
        return neutra_core::mounts::system_mounts()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| {
                m.network_host()
                    .map(|h| (h, format!("{}:{}", m.device, m.mountpoint.display())))
            })
            .collect();
    }
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("/sbin/mount").output().ok();
        return out
            .into_iter()
            .flat_map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter_map(|l| {
                let (spec, rest) = l.split_once(" on ")?;
                if !(rest.contains("nfs") || rest.contains("smbfs") || rest.contains("webdav")) {
                    return None;
                }
                let host = spec
                    .trim_start_matches("//")
                    .rsplit('@')
                    .next()?
                    .split([':', '/'])
                    .next()?
                    .to_string();
                Some((host, l))
            })
            .collect();
    }
    #[cfg(target_os = "windows")]
    {
        let out = Command::new("net").arg("use").output().ok();
        return out
            .into_iter()
            .flat_map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter_map(|l| {
                let unc = l.split_whitespace().find(|s| s.starts_with(r"\\"))?;
                let host = unc.trim_start_matches('\\').split('\\').next()?.to_string();
                Some((host, l))
            })
            .collect();
    }
    #[allow(unreachable_code)]
    Vec::new()
}
