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
            .or_else(|| std::env::var_os("NEUTRA_HELPER_ARTIFACTS"))
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
            bail!("SSH works neither as Unix shell nor Windows OpenSSH on {host}");
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
fn remote_helper_path(p: &RemotePlatform) -> &'static str {
    match p.os {
        RemoteOs::Windows => r#"$env:LOCALAPPDATA\Neutrasearch\neutrasearch-helper.exe"#,
        _ => "~/.local/lib/neutrasearch/neutrasearch-helper",
    }
}

fn windows_command(script: &str) -> String {
    format!("powershell -NoProfile -NonInteractive -Command \"{script}\"")
}

fn build_check_command(p: &RemotePlatform) -> String {
    match p.os {
        RemoteOs::Windows => windows_command(
            r#"& (Join-Path $env:LOCALAPPDATA 'Neutrasearch\neutrasearch-helper.exe') --build"#,
        ),
        // Do not quote this path: the remote Unix shell must expand `~`.
        _ => format!("{} --build", remote_helper_path(p)),
    }
}

fn connect_command(p: &RemotePlatform) -> String {
    match p.os {
        RemoteOs::Windows => windows_command(
            r#"& (Join-Path $env:LOCALAPPDATA 'Neutrasearch\neutrasearch-helper.exe')"#,
        ),
        _ => remote_helper_path(p).into(),
    }
}

fn unix_install_command(expected_sha256: &str, os: RemoteOs) -> String {
    let hash_command = match os {
        RemoteOs::Linux => "sha256sum",
        RemoteOs::Macos => "shasum -a 256",
        RemoteOs::Windows => unreachable!("Windows uses a PowerShell install command"),
    };
    format!(
        "actual=$({hash_command} ~/.local/lib/neutrasearch/neutrasearch-helper.new | cut -d' ' -f1); test \"$actual\" = '{expected_sha256}' && chmod 700 ~/.local/lib/neutrasearch/neutrasearch-helper.new && mv ~/.local/lib/neutrasearch/neutrasearch-helper.new ~/.local/lib/neutrasearch/neutrasearch-helper"
    )
}

fn windows_install_command(expected_sha256: &str) -> String {
    windows_command(&format!(
        "$source = Join-Path $HOME 'neutrasearch-helper.exe.new'; $actual = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant(); if ($actual -ne '{expected_sha256}') {{ Remove-Item -LiteralPath $source -Force; exit 42 }}; Move-Item -LiteralPath $source -Destination (Join-Path $env:LOCALAPPDATA 'Neutrasearch\\neutrasearch-helper.exe') -Force"
    ))
}

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
fn normalize_arch(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" | "x64" => "x86_64".into(),
        "aarch64" | "arm64" => "aarch64".into(),
        x => x.into(),
    }
}
fn validate_host(host: &str) -> Result<()> {
    let (user, hostname) = match host.split_once('@') {
        Some((user, hostname)) if !hostname.contains('@') => (Some(user), hostname),
        Some(_) => bail!("unsafe SSH host value"),
        None => (None, host),
    };

    if let Some(user) = user {
        if user.is_empty()
            || user.starts_with('-')
            || !user
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        {
            bail!("unsafe SSH host value");
        }
    }

    let valid_hostname =
        if hostname.len() > 2 && hostname.starts_with('[') && hostname.ends_with(']') {
            let address = &hostname[1..hostname.len() - 1];
            address.contains(':')
                && address
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        } else {
            !hostname.is_empty()
                && !hostname.starts_with('-')
                && hostname
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        };
    if !valid_hostname {
        bail!("unsafe SSH host value");
    }
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
    fn platform(os: RemoteOs, arch: &str) -> RemotePlatform {
        RemotePlatform {
            os,
            arch: arch.into(),
        }
    }

    #[test]
    fn artifact_matrix() {
        assert_eq!(
            artifact_name(&platform(RemoteOs::Linux, "x86_64")),
            "neutrasearch-helper-linux-x86_64"
        );
        assert_eq!(
            artifact_name(&platform(RemoteOs::Windows, "x86_64")),
            "neutrasearch-helper-windows-x86_64.exe"
        );
        assert_eq!(
            artifact_name(&platform(RemoteOs::Macos, "aarch64")),
            "neutrasearch-helper-macos-aarch64"
        );
    }

    #[test]
    fn platform_commands_expand_paths_in_the_remote_shell() {
        let linux = platform(RemoteOs::Linux, "x86_64");
        assert_eq!(
            build_check_command(&linux),
            "~/.local/lib/neutrasearch/neutrasearch-helper --build"
        );
        assert_eq!(
            connect_command(&linux),
            "~/.local/lib/neutrasearch/neutrasearch-helper"
        );

        let windows = platform(RemoteOs::Windows, "x86_64");
        assert_eq!(
            build_check_command(&windows),
            "powershell -NoProfile -NonInteractive -Command \"& (Join-Path $env:LOCALAPPDATA 'Neutrasearch\\neutrasearch-helper.exe') --build\""
        );
        assert_eq!(
            connect_command(&windows),
            "powershell -NoProfile -NonInteractive -Command \"& (Join-Path $env:LOCALAPPDATA 'Neutrasearch\\neutrasearch-helper.exe')\""
        );
        let digest = "a".repeat(64);
        assert!(windows_install_command(&digest).contains("Join-Path $HOME"));
        assert!(windows_install_command(&digest).contains("Move-Item"));
        assert!(windows_install_command(&digest).contains("$env:LOCALAPPDATA"));
        assert!(windows_install_command(&digest).contains(&digest));
        assert!(unix_install_command(&digest, RemoteOs::Linux).contains("sha256sum"));
        assert!(unix_install_command(&digest, RemoteOs::Macos).contains("shasum -a 256"));
    }

    #[test]
    fn local_artifact_requires_matching_sha256_sidecar() {
        let path = std::env::temp_dir().join(format!(
            "neutrasearch-remote-artifact-{}",
            std::process::id()
        ));
        let checksum = append_suffix(&path, ".sha256");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&checksum);
        std::fs::write(&path, b"helper bytes").unwrap();
        assert!(verify_local_artifact(&path).is_err());
        let digest = format!("{:x}", Sha256::digest(b"helper bytes"));
        std::fs::write(&checksum, format!("{digest}  helper\n")).unwrap();
        assert_eq!(verify_local_artifact(&path).unwrap(), digest);
        std::fs::write(&checksum, format!("{}  helper\n", "0".repeat(64))).unwrap();
        assert!(verify_local_artifact(&path).is_err());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(checksum).unwrap();
    }

    #[test]
    fn accepts_unambiguous_ssh_hosts() {
        for host in [
            "good-host",
            "server.example.test",
            "alice@good-host",
            "build_bot@10.0.0.4",
            "[2001:db8::1]",
            "alice@[2001:db8::1]",
        ] {
            assert!(validate_host(host).is_ok(), "should accept {host}");
        }
    }

    #[test]
    fn rejects_option_path_and_shell_ambiguous_hosts() {
        for host in [
            "",
            "-oProxyCommand=bad",
            "alice@-bad",
            "alice@@host",
            "host:path",
            "host/path",
            "host\\path",
            "bad;rm",
            "bad host",
            "[not-ipv6]",
            "[]",
            "[",
        ] {
            assert!(validate_host(host).is_err(), "should reject {host}");
        }
    }
}
