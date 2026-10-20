//! Resident Windows/macOS shortcuts, with live settings reload and status receipts.

use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::{collections::HashMap, process::Command, time::Duration};

#[cfg(target_os = "windows")]
#[path = "native/windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "native/macos.rs"]
mod platform;

const RELOAD_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) fn run() -> Result<(), String> {
    platform::run_agent()
}

pub(crate) fn install_autostart() -> Result<(), String> {
    platform::install_autostart()
}

#[cfg(target_os = "macos")]
pub(super) fn command(command: &mut Command) -> Result<std::process::Output, String> {
    use std::{process::Stdio, time::Instant};
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            return child.wait_with_output().map_err(|e| e.to_string());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Launch-agent registration timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Resident agent loop. Never returns under normal operation.
///
/// The manager and platform message pump run on the same thread.
pub(super) fn agent_loop(mut wait_for_events: impl FnMut(Duration)) -> Result<(), String> {
    let manager = GlobalHotKeyManager::new()
        .map_err(|error| format!("cannot create global shortcut manager: {error}"))?;
    let events = GlobalHotKeyEvent::receiver();
    let mut active = Vec::<HotKey>::new();
    let mut known = HashMap::<u32, String>::new();
    let mut requested: Option<Vec<String>> = None;
    let mut status_error = None::<String>;
    let mut last_revision: Option<ConfigRevision> = None;

    loop {
        // A new file revision means a new request, even when the normalized
        // keys equal the previous request: the parent deletes status.json
        // before saving, so every revision needs a fresh receipt.
        let revision = config_revision();
        let status_missing = !super::config_path().with_extension("status.json").exists();
        if status_missing || last_revision.as_ref() != Some(&revision) {
            last_revision = Some(revision);
            match super::load() {
                Err(error) => {
                    status_error = Some(format!("cannot load shortcut settings: {error}"));
                }
                Ok(keys) => {
                    requested = Some(keys.clone());
                    match parse_keys(&keys) {
                        Err(error) => status_error = Some(error),
                        Ok(candidate) => {
                            match apply_registrations(&manager, &mut active, &candidate) {
                                Ok(()) => {
                                    known = active
                                        .iter()
                                        .zip(&keys)
                                        .map(|(hotkey, key)| (hotkey.id(), key.clone()))
                                        .collect();
                                    status_error = None;
                                }
                                Err(error) => status_error = Some(error),
                            }
                        }
                    }
                }
            }
            let keys = requested.clone().unwrap_or_default();
            write_status(&keys, status_error.as_deref());
        }

        let mut launch_error = None::<String>;
        while let Ok(event) = events.try_recv() {
            if event.state() == HotKeyState::Pressed && known.contains_key(&event.id()) {
                match std::env::current_exe() {
                    Err(error) => {
                        launch_error = Some(format!("cannot resolve executable path: {error}"));
                    }
                    Ok(executable) => {
                        if let Err(error) = Command::new(executable).arg("--spotlight").spawn() {
                            launch_error = Some(format!("cannot launch quick search: {error}"));
                        }
                    }
                }
            }
        }
        if let Some(message) = launch_error {
            if status_error.as_deref() != Some(&message) {
                status_error = Some(message);
                let keys = requested.clone().unwrap_or_default();
                write_status(&keys, status_error.as_deref());
            }
        }

        wait_for_events(RELOAD_INTERVAL);
    }
}

fn apply_registrations(
    manager: &GlobalHotKeyManager,
    active: &mut Vec<HotKey>,
    candidate: &[HotKey],
) -> Result<(), String> {
    super::registration::update(
        active,
        candidate,
        |key| manager.register(key).map_err(|e| e.to_string()),
        |key| manager.unregister(key).map_err(|e| e.to_string()),
    )
}

/// Empty is legal and means "disable all shortcuts".
fn parse_keys(keys: &[String]) -> Result<Vec<HotKey>, String> {
    let mut parsed = Vec::with_capacity(keys.len());
    for key in keys {
        let hotkey = key
            .parse::<HotKey>()
            .map_err(|error| format!("invalid shortcut '{key}': {error}"))?;
        if parsed
            .iter()
            .any(|other: &HotKey| other.id() == hotkey.id())
        {
            return Err(format!("duplicate global shortcut '{key}'"));
        }
        parsed.push(hotkey);
    }
    Ok(parsed)
}

/// File revision of the persisted config. The parent saves atomically (write
/// temp + rename), so deletion/recreation and every save change what this
/// observes even when the normalized keys are unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConfigRevision {
    modified: Option<(u64, u32)>,
    len: u64,
    exists: bool,
}

fn config_revision() -> ConfigRevision {
    match std::fs::metadata(super::config_path()) {
        Err(_) => ConfigRevision {
            modified: None,
            len: 0,
            exists: false,
        },
        Ok(meta) => ConfigRevision {
            modified: meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|age| (age.as_secs(), age.subsec_nanos())),
            len: meta.len(),
            exists: true,
        },
    }
}

fn write_status(keys: &[String], error: Option<&str>) {
    #[derive(serde::Serialize)]
    struct Status<'a> {
        keys: &'a [String],
        error: Option<&'a str>,
    }

    let path = super::config_path().with_extension("status.json");
    let result = (|| -> std::io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("status path has no parent"))?;
        std::fs::create_dir_all(parent)?;
        let temporary = path.with_extension(format!("status-{}.tmp", std::process::id()));
        std::fs::write(
            &temporary,
            serde_json::to_vec(&Status { keys, error }).map_err(std::io::Error::other)?,
        )?;
        crate::settings::publish_settings(&temporary, &path)
    })();
    if let Err(error) = result {
        eprintln!("neutrasearch: cannot write shortcut status: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_shortcuts_and_rejects_duplicate_bindings() {
        assert_eq!(
            parse_keys(&["Alt+KeyK".into(), "Control+Shift+KeyJ".into()])
                .unwrap()
                .len(),
            2
        );
        assert!(parse_keys(&["Alt+KeyK".into(), "Alt+KeyK".into()])
            .unwrap_err()
            .contains("duplicate"));
    }

    #[test]
    fn empty_keys_disable_all_shortcuts() {
        assert!(parse_keys(&[]).unwrap().is_empty());
    }
}
