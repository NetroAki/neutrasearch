//! Remote-index lane for NFS/SMB/SSHFS mounts.
//!
//! Network mounts do not expose a local filesystem index. Neutrasearch never
//! walks them from the client; it installs the matching helper on the server
//! over the user's existing SSH key/agent and streams the same framed protocol.

use anyhow::{bail, Context, Result};
use neutra_core::proto::HELPER_BUILD;
use neutra_core::{MountInfo, MountSource};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteOs {
    Linux,
    Windows,
    Macos,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePlatform {
    pub os: RemoteOs,
    pub arch: String,
}

#[derive(Debug, Clone)]
pub struct Provisioner {
    /// Directory containing prebuilt helper artifacts named by
    /// `artifact_name()`. Each portable archive includes its own target;
    /// administrators can add other release targets for heterogeneous servers.
    pub artifacts: PathBuf,
    pub connect_timeout_secs: u32,
}

impl Provisioner {
    pub fn from_env() -> Self {
        let artifacts = std::env::var_os("NEUTRASEARCH_HELPER_ARTIFACTS")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(|parent| parent.join("helpers")))
                    .unwrap_or_else(|| PathBuf::from("helpers"))
            });
        Self {
            artifacts,
            connect_timeout_secs: 5,
        }
    }

    pub fn detect(&self, host: &str) -> Result<RemotePlatform> {
        validate_host(host)?;
        let unix = self.ssh(host).arg("uname -s; uname -m").output()?;
        if unix.status.success() {
            let s = String::from_utf8_lossy(&unix.stdout);
            let mut l = s.lines();
            let os = match l.next().unwrap_or("").trim() {
                "Linux" => RemoteOs::Linux,
                "Darwin" => RemoteOs::Macos,
                other => bail!("unsupported remote OS {other}"),
            };
            return Ok(RemotePlatform {
                os,
                arch: normalize_arch(l.next().unwrap_or("")),
            });
        }
        let win = self
            .ssh(host)
            .args([
                "powershell",
                "-NoProfile",
                "-Command",
                "[System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture",
            ])
            .output()?;
        if !win.status.success() {
            let unix_error = String::from_utf8_lossy(&unix.stderr);
            let windows_error = String::from_utf8_lossy(&win.stderr);
            let detail = [unix_error.trim(), windows_error.trim()]
                .into_iter()
                .filter(|message| !message.is_empty())
                .collect::<Vec<_>>()
                .join("; ");
            if detail.is_empty() {
                bail!("remote helper unavailable on {host}");
            }
            bail!("remote helper unavailable on {host}: {detail}");
        }
        Ok(RemotePlatform {
            os: RemoteOs::Windows,
            arch: normalize_arch(&String::from_utf8_lossy(&win.stdout)),
        })
    }

    pub fn ensure_installed(&self, host: &str) -> Result<RemotePlatform> {
        let p = self.detect(host)?;
        let check = self.ssh(host).arg(build_check_command(&p)).output()?;
        if check.status.success()
            && String::from_utf8_lossy(&check.stdout).trim() == HELPER_BUILD.to_string()
        {
            return Ok(p);
        }
        let artifact = self.artifacts.join(artifact_name(&p));
        if !artifact.is_file() {
            bail!("missing prebuilt helper {} for {host}", artifact.display());
        }
        let digest = verify_local_artifact(&artifact)?;
        match p.os {
            RemoteOs::Linux | RemoteOs::Macos => {
                checked(
                    self.ssh(host).arg("mkdir -p ~/.local/lib/neutrasearch"),
                    "create remote helper dir",
                )?;
                let target = format!("{host}:~/.local/lib/neutrasearch/neutrasearch-helper.new");
                checked(
                    Command::new("scp")
                        .args(self.ssh_options())
                        .arg(&artifact)
                        .arg(&target),
                    "scp helper",
                )?;
                checked(
                    self.ssh(host).arg(unix_install_command(&digest, p.os)),
                    "verify and install helper",
                )?;
            }
            RemoteOs::Windows => {
                checked(
                    self.ssh(host).arg(windows_command(
                        "New-Item -ItemType Directory -Force -Path $env:LOCALAPPDATA\\Neutrasearch | Out-Null",
                    )),
                    "create remote Windows helper dir",
                )?;
                // Upload into the SSH home first: scp cannot expand PowerShell
                // environment variables in its destination path.
                let target = format!("{host}:neutrasearch-helper.exe.new");
                checked(
                    Command::new("scp")
                        .args(self.ssh_options())
                        .arg(&artifact)
                        .arg(&target),
                    "scp Windows helper",
                )?;
                checked(
                    self.ssh(host).arg(windows_install_command(&digest)),
                    "verify and install Windows helper",
                )?;
            }
        }
        Ok(p)
    }

    /// Start a protocol channel. Caller writes ClientMsg frames to stdin and
    /// reads HelperMsg frames from stdout.
    pub fn connect(&self, host: &str) -> Result<Child> {
        let p = self.ensure_installed(host)?;
        self.ssh(host)
            .arg(connect_command(&p))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("start remote neutrasearch-helper")
    }

    pub fn provision_mount(&self, mount: &MountInfo) -> Result<RemotePlatform> {
        let host = match &mount.source {
            MountSource::Remote { host } if !host.is_empty() => host,
            _ => bail!("mount is not a remote source"),
        };
        self.ensure_installed(host)
    }

    fn ssh(&self, host: &str) -> Command {
        let mut c = Command::new("ssh");
        c.args(self.ssh_options()).arg(host);
        c
    }
    fn ssh_options(&self) -> [String; 6] {
        [
            "-o".into(),
            "BatchMode=yes".into(),
            // Never accept an unknown host key silently while provisioning.
            "-o".into(),
            "StrictHostKeyChecking=yes".into(),
            "-o".into(),
            format!("ConnectTimeout={}", self.connect_timeout_secs),
        ]
    }
}

#[path = "lib/commands.rs"]
mod commands;
pub use commands::artifact_name;
use commands::{
    build_check_command, connect_command, unix_install_command, windows_command,
    windows_install_command,
};

fn verify_local_artifact(path: &Path) -> Result<String> {
    let checksum_path = append_suffix(path, ".sha256");
    let expected = std::fs::read_to_string(&checksum_path)
        .with_context(|| format!("read helper checksum {}", checksum_path.display()))?;
    let expected = expected
        .split_whitespace()
        .next()
        .context("helper checksum file is empty")?
        .to_ascii_lowercase();
    if expected.len() != 64
        || !expected
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        bail!("invalid SHA-256 in {}", checksum_path.display());
    }
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        bail!("helper checksum mismatch for {}", path.display());
    }
    Ok(actual)
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
}
#[path = "lib/validation.rs"]
mod validation;
use validation::{normalize_arch, validate_host};

fn checked(cmd: &mut Command, what: &str) -> Result<()> {
    let s = cmd.status()?;
    if !s.success() {
        bail!("{what} failed with {s}")
    };
    Ok(())
}

/// Polling helper used by GUI startup/watch code. It returns only newly seen
/// network mount identities; it does not perform any VFS access to them.
pub fn new_network_mounts<'a>(
    mounts: &'a [MountInfo],
    seen: &mut std::collections::HashSet<String>,
) -> Vec<&'a MountInfo> {
    mounts
        .iter()
        .filter(|m| matches!(m.source, MountSource::Remote { .. }))
        .filter(|m| seen.insert(format!("{}:{}", m.device, m.mountpoint.display())))
        .collect()
}

#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;
