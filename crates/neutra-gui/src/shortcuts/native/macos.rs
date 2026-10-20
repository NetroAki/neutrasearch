use std::process::Command;
#[path = "macos/events.rs"]
mod events;

pub(super) fn run_agent() -> Result<(), String> {
    let _lock = acquire_lock()?;
    events::run()
}

pub(super) fn install_autostart() -> Result<(), String> {
    let executable =
        std::env::current_exe().map_err(|e| format!("cannot resolve executable: {e}"))?;
    let directory = std::env::var_os("HOME")
        .ok_or("HOME is unavailable")?
        .into_string()
        .map_err(|_| "HOME is not valid UTF-8")?;
    let agents = std::path::PathBuf::from(directory).join("Library/LaunchAgents");
    std::fs::create_dir_all(&agents).map_err(|e| format!("cannot create LaunchAgents: {e}"))?;
    let plist = agents.join("com.neutrasearch.shortcut-agent.plist");
    let content = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>com.neutrasearch.shortcut-agent</string><key>ProgramArguments</key><array><string>{}</string><string>--shortcut-agent</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/></dict></plist>\n", xml_escape(&executable.to_string_lossy()));
    let domain = format!("gui/{}", unsafe { libc::geteuid() });
    let service = format!("{domain}/com.neutrasearch.shortcut-agent");
    if std::fs::read_to_string(&plist).ok().as_deref() == Some(&content)
        && super::command(Command::new("launchctl").args(["print", &service]))?
            .status
            .success()
    {
        return Ok(());
    }
    let temporary = plist.with_extension("plist.tmp");
    std::fs::write(&temporary, content).map_err(|e| format!("cannot write launch agent: {e}"))?;
    std::fs::rename(&temporary, &plist).map_err(|e| format!("cannot install launch agent: {e}"))?;
    let _ = super::command(Command::new("launchctl").args([
        "bootout",
        &domain,
        plist.to_string_lossy().as_ref(),
    ]));
    let output = super::command(Command::new("launchctl").args([
        "bootstrap",
        &domain,
        plist.to_string_lossy().as_ref(),
    ]))?;
    if !output.status.success() {
        return Err(format!(
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let kick = super::command(Command::new("launchctl").args(["kickstart", &service]))?;
    if !kick.status.success() {
        return Err(format!(
            "launchctl kickstart failed: {}",
            String::from_utf8_lossy(&kick.stderr).trim()
        ));
    }
    Ok(())
}

fn acquire_lock() -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut path = super::super::config_path();
    path.set_extension("lock");
    std::fs::create_dir_all(path.parent().ok_or("Shortcut lock has no parent")?)
        .map_err(|e| e.to_string())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| format!("cannot open shortcut-agent lock: {e}"))?;
    let result = unsafe {
        libc::flock(
            std::os::fd::AsRawFd::as_raw_fd(&file),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    };
    if result != 0 {
        return Err("another shortcut agent already owns the configuration".into());
    }
    Ok(file)
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::xml_escape;
    #[test]
    fn escapes_launch_agent_paths_as_xml_text() {
        assert_eq!(
            xml_escape("/Users/a&b/<app>.app/\"x\""),
            "/Users/a&amp;b/&lt;app&gt;.app/&quot;x&quot;"
        );
    }
}
