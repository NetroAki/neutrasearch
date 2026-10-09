//! Desktop application lookup for Open With (gio mime on Linux).
use super::run;
use std::path::Path;

pub(crate) fn desktop_apps(path: &Path) -> Vec<crate::app::file_operations::DesktopApp> {
    #[cfg(target_os = "linux")]
    {
        let mime = run(
            "gio",
            &[
                "info",
                "-a",
                "standard::content-type",
                "--",
                &path.to_string_lossy(),
            ],
        );
        let mime = mime
            .ok()
            .and_then(|s| {
                s.lines().find_map(|l| {
                    l.trim()
                        .strip_prefix("standard::content-type: ")
                        .map(str::to_owned)
                })
            })
            .unwrap_or_else(|| "application/octet-stream".into());
        let Ok(out) = run("gio", &["mime", &mime]) else {
            return Vec::new();
        };
        let text = out;
        let mut apps = Vec::new();
        let mut collect = false;
        for line in text.lines() {
            if line.starts_with("Default application:") {
                let id = line
                    .split_once(':')
                    .map(|(_, v)| v.trim())
                    .unwrap_or_default();
                if id.ends_with(".desktop") {
                    apps.push(crate::app::file_operations::DesktopApp {
                        id: id.into(),
                        name: id.trim_end_matches(".desktop").replace(['-', '_'], " "),
                    });
                }
            }
            if line.starts_with("Recommended applications:") {
                collect = true;
                continue;
            }
            if collect {
                for id in line.split_whitespace() {
                    if id.ends_with(".desktop")
                        && !apps
                            .iter()
                            .any(|a: &crate::app::file_operations::DesktopApp| a.id == id)
                    {
                        apps.push(crate::app::file_operations::DesktopApp {
                            id: id.into(),
                            name: id.trim_end_matches(".desktop").replace(['-', '_'], " "),
                        });
                    }
                }
            }
        }
        apps
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        Vec::new()
    }
}
