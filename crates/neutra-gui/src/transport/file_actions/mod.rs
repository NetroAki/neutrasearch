use self::clipboard::{get_clipboard, set_clipboard};
use self::trash::{restore, trash};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Open(PathBuf),
    OpenWith {
        path: PathBuf,
        desktop_id: String,
    },
    Reveal(PathBuf),
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    Trash(PathBuf),
    TrashMany(Vec<PathBuf>),
    RestoreMany(Vec<PathBuf>),
    Restore(PathBuf),
    Copy(Vec<PathBuf>),
    Cut(Vec<PathBuf>),
    Paste(PathBuf),
    Transfer {
        paths: Vec<PathBuf>,
        destination: PathBuf,
        cut: bool,
    },
}
#[path = "apps.rs"]
mod apps;
pub(crate) use apps::desktop_apps;

pub(crate) fn execute(action: Action) -> Result<Option<PathBuf>, String> {
    match action {
        Action::Open(p) => launch(&p, None),
        Action::Reveal(p) => reveal(&p),
        Action::OpenWith { path, desktop_id } => {
            if desktop_id.is_empty() {
                return Err("choose an application first".into());
            }
            launch(&path, Some(&desktop_id))
        }
        Action::Rename { from, to } => {
            rename_noreplace(&from, &to)?;
            Ok(Some(to))
        }
        Action::Trash(path) => {
            let id = trash(&path)?;
            Ok(Some(id))
        }
        Action::TrashMany(_) | Action::RestoreMany(_) => {
            Err("batch trash must be queued as individual undoable operations".into())
        }
        Action::Restore(uri) => restore(&uri).map(Some),
        Action::Copy(paths) => set_clipboard(&paths, false).map(|_| None),
        Action::Cut(paths) => set_clipboard(&paths, true).map(|_| None),
        Action::Paste(target) => {
            let dest = if target.is_dir() {
                target
            } else {
                target.parent().unwrap_or(Path::new(".")).to_path_buf()
            };
            let (cut, paths) = get_clipboard()?;
            transfer_files(&paths, &dest, cut)?;
            Ok(Some(dest))
        }
        Action::Transfer {
            paths,
            destination,
            cut,
        } => {
            transfer_files(&paths, &destination, cut)?;
            Ok(Some(destination))
        }
    }
}

#[path = "transfer.rs"]
mod transfer;
pub(crate) use transfer::transfer as transfer_files;

#[cfg(target_os = "linux")]
pub(super) fn rename_noreplace(from: &Path, to: &Path) -> Result<(), String> {
    transfer::rename_noreplace(from, to)
}
#[cfg(not(target_os = "linux"))]
pub(super) fn rename_noreplace(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        return Err(format!("destination exists: {}", to.display()));
    }
    std::fs::rename(from, to).map_err(|e| e.to_string())
}

fn launch(path: &Path, desktop: Option<&str>) -> Result<Option<PathBuf>, String> {
    #[cfg(target_os = "linux")]
    let mut c = if let Some(id) = desktop {
        let mut c = Command::new("gtk-launch");
        c.arg(id).arg(path);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut c = {
        let mut c = Command::new("open");
        if let Some(id) = desktop {
            c.arg("-a").arg(id);
        }
        c.arg(path);
        c
    };
    #[cfg(target_os = "windows")]
    let mut c = {
        let mut c = Command::new("explorer.exe");
        c.arg(path);
        c
    };
    let child = c
        .spawn()
        .map_err(|e| format!("cannot launch desktop application: {e}"))?;
    drop(child);
    Ok(None)
}

fn reveal(path: &Path) -> Result<Option<PathBuf>, String> {
    #[cfg(target_os = "linux")]
    {
        let mut c = Command::new("nautilus");
        c.arg("--select").arg(path);
        match c.spawn() {
            Ok(child) => {
                drop(child);
                Ok(None)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                launch(path.parent().unwrap_or(path), None)
            }
            Err(error) => Err(format!("cannot reveal file in file manager: {error}")),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        c.spawn()
            .map(|child| {
                drop(child);
                None
            })
            .map_err(|e| format!("cannot reveal file: {e}"))
    }
    #[cfg(target_os = "windows")]
    {
        let mut c = Command::new("explorer.exe");
        c.arg(format!("/select,{}", path.display()));
        c.spawn()
            .map(|child| {
                drop(child);
                None
            })
            .map_err(|e| format!("cannot reveal file: {e}"))
    }
}
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    run_with_env(program, args, None)
}
fn run_with_env(program: &str, args: &[&str], data_home: Option<&Path>) -> Result<String, String> {
    use std::{io::Read, process::Stdio};
    // The desktop GVfs daemon retains its original data directory. An
    // isolated data home needs its own bus so Trash lookup uses that home.
    let mut c = if data_home.is_some() {
        let mut command = Command::new("dbus-run-session");
        command.args(["--", program]);
        command
    } else {
        Command::new(program)
    };
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(home) = data_home {
        c.env("XDG_DATA_HOME", home);
    }
    let mut child = c
        .spawn()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let out_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = std::io::BufReader::new(stdout).read_to_end(&mut bytes);
        (result, bytes)
    });
    let err_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = std::io::BufReader::new(stderr).read_to_end(&mut bytes);
        (result, bytes)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_reader.join();
                let _ = err_reader.join();
                return Err(format!("{program} timed out"));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_reader.join();
                let _ = err_reader.join();
                return Err(format!("cannot wait for {program}: {e}"));
            }
        }
    };
    let (read_out, out) = out_reader
        .join()
        .map_err(|_| format!("cannot read {program} output"))?;
    let (read_err, err) = err_reader
        .join()
        .map_err(|_| format!("cannot read {program} error output"))?;
    read_out.map_err(|error| format!("cannot read {program} output: {error}"))?;
    read_err.map_err(|error| format!("cannot read {program} error output: {error}"))?;
    if !status.success() {
        Err(format!(
            "{program}: {}",
            String::from_utf8_lossy(&err).trim()
        ))
    } else {
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}

#[path = "clipboard.rs"]
mod clipboard;
mod trash;

#[cfg(test)]
mod tests;
