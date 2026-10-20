mod config;
#[cfg(target_os = "linux")]
mod kde;
pub(crate) mod keys;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod native;
#[cfg(any(target_os = "windows", target_os = "macos", test))]
mod registration;
mod state;

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) use config::config_path;
pub(crate) use config::load;
pub(crate) use keys::normalize;
pub(crate) use state::ShortcutSettings;

pub(crate) const DEFAULT: &str = "alt+KeyK";

pub(crate) fn command() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = match args.first().and_then(|s| s.to_str()) {
        Some("--register-shortcuts") if args.len() == 1 => register_saved(),
        Some("--shortcut-agent") if args.len() == 1 => run_agent(),
        _ => return None,
    };
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("neutrasearch: {error}");
            1
        }
    })
}

pub(crate) fn register_saved() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        kde::register(&load()?)
    }
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        native::install_autostart()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    Err("Global shortcuts are unavailable on this platform".into())
}

pub(crate) fn run_agent() -> Result<(), String> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        native::run()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    Err("This desktop uses its own global shortcut service".into())
}

pub(crate) fn apply(keys: &[String]) -> Result<(), String> {
    let keys = normalize(keys)?;
    #[cfg(target_os = "linux")]
    {
        let previous = config::snapshot()?;
        config::save(&keys)?;
        if let Err(error) = kde::register(&keys) {
            return match config::restore(previous) {
                Ok(()) => Err(error),
                Err(rollback) => Err(format!(
                    "{error}; cannot restore saved shortcuts: {rollback}"
                )),
            };
        }
        Ok(())
    }
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        config::apply_native(&keys)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    Err("Global shortcuts are unavailable on this platform".into())
}
