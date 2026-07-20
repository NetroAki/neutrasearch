//! Remote-index lane for NFS/SMB/SSHFS mounts.
//!
//! Network mounts do not expose a local filesystem index. Neutrasearch never
//! walks them from the client; it installs the matching helper on the server
//! over the user's existing SSH key/agent and streams the same framed protocol.

use anyhow::{bail, Context, Result};
use neutra_core::proto::HELPER_BUILD;
use neutra_core::{MountInfo, MountSource};
use std::path::PathBuf;
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
    /// `artifact_name()`. Packaging places all three OS variants here.
    pub artifacts: PathBuf,
    pub connect_timeout_secs: u32,
}

impl Provisioner {
    pub fn from_env() -> Self {
        let artifacts = std::env::var_os("NEUTRASEARCH_HELPER_ARTIFACTS")
            .or_else(|| std::env::var_os("NEUTRA_HELPER_ARTIFACTS"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("helpers"));
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
            bail!("SSH works neither as Unix shell nor Windows OpenSSH on {host}");
        }
        Ok(RemotePlatform {
            os: RemoteOs::Windows,
            arch: normalize_arch(&String::from_utf8_lossy(&win.stdout)),
        })
    }

    pub fn ensure_installed(&self, host: &str) -> Result<RemotePlatform> {
        let p = self.detect(host)?;
        let remote = remote_helper_path(&p);
        let check = self
            .ssh(host)
            .arg(format!("\"{remote}\" --build"))
            .output()?;
        if check.status.success()
            && String::from_utf8_lossy(&check.stdout).trim() == HELPER_BUILD.to_string()
        {
            return Ok(p);
        }
        let artifact = self.artifacts.join(artifact_name(&p));
        if !artifact.is_file() {
            bail!("missing prebuilt helper {} for {host}", artifact.display());
        }
        match p.os {
            RemoteOs::Linux | RemoteOs::Macos => {
                self.ssh(host)
                    .arg("mkdir -p ~/.local/lib/neutrasearch")
                    .status()
                    .context("create remote helper dir")?;
                let target = format!("{host}:~/.local/lib/neutrasearch/neutrasearch-helper.new");
                checked(
                    Command::new("scp")
                        .args(self.ssh_options())
                        .arg(&artifact)
                        .arg(&target),
                    "scp helper",
                )?;
                checked(self.ssh(host).arg("chmod 700 ~/.local/lib/neutrasearch/neutrasearch-helper.new && mv ~/.local/lib/neutrasearch/neutrasearch-helper.new ~/.local/lib/neutrasearch/neutrasearch-helper"),"install helper")?;
            }
            RemoteOs::Windows => {
                self.ssh(host).args(["powershell","-NoProfile","-Command","New-Item -ItemType Directory -Force $env:LOCALAPPDATA\\Neutrasearch | Out-Null"]).status()?;
                let target = format!("{host}:Neutrasearch/neutrasearch-helper.exe");
                checked(
                    Command::new("scp")
                        .args(self.ssh_options())
                        .arg(&artifact)
                        .arg(&target),
                    "scp Windows helper",
                )?;
            }
        }
        Ok(p)
    }

    /// Start a protocol channel. Caller writes ClientMsg frames to stdin and
    /// reads HelperMsg frames from stdout.
    pub fn connect(&self, host: &str) -> Result<Child> {
        let p = self.ensure_installed(host)?;
        let remote = remote_helper_path(&p);
        self.ssh(host)
            .arg(remote)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
    fn ssh_options(&self) -> [String; 4] {
        [
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            format!("ConnectTimeout={}", self.connect_timeout_secs),
        ]
    }
}

pub fn artifact_name(p: &RemotePlatform) -> String {
    let os = match p.os {
        RemoteOs::Linux => "linux",
        RemoteOs::Windows => "windows",
        RemoteOs::Macos => "macos",
    };
    let ext = if p.os == RemoteOs::Windows {
        ".exe"
    } else {
        ""
    };
    format!("neutrasearch-helper-{os}-{}{ext}", p.arch)
}
fn remote_helper_path(p: &RemotePlatform) -> String {
    match p.os {
        RemoteOs::Windows => r#"%LOCALAPPDATA%\Neutrasearch\neutrasearch-helper.exe"#.into(),
        _ => "~/.local/lib/neutrasearch/neutrasearch-helper".into(),
    }
}
fn normalize_arch(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" | "x64" => "x86_64".into(),
        "aarch64" | "arm64" => "aarch64".into(),
        x => x.into(),
    }
}
fn validate_host(host: &str) -> Result<()> {
    if host.is_empty()
        || host.starts_with('-')
        || host
            .chars()
            .any(|c| c.is_whitespace() || ";&|`$".contains(c))
    {
        bail!("unsafe SSH host value")
    };
    Ok(())
}
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
mod tests {
    use super::*;
    #[test]
    fn artifact_matrix() {
        assert_eq!(
            artifact_name(&RemotePlatform {
                os: RemoteOs::Windows,
                arch: "x86_64".into()
            }),
            "neutrasearch-helper-windows-x86_64.exe"
        );
        assert_eq!(
            artifact_name(&RemotePlatform {
                os: RemoteOs::Macos,
                arch: "aarch64".into()
            }),
            "neutrasearch-helper-macos-aarch64"
        );
    }
    #[test]
    fn rejects_shell_hosts() {
        assert!(validate_host("good-host").is_ok());
        assert!(validate_host("bad;rm").is_err());
    }
}
