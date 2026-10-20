use super::*;

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
pub(super) fn remote_helper_path(p: &RemotePlatform) -> &'static str {
    match p.os {
        RemoteOs::Windows => r#"$env:LOCALAPPDATA\Neutrasearch\neutrasearch-helper.exe"#,
        _ => "~/.local/lib/neutrasearch/neutrasearch-helper",
    }
}

pub(super) fn windows_command(script: &str) -> String {
    format!("powershell -NoProfile -NonInteractive -Command \"{script}\"")
}

pub(super) fn build_check_command(p: &RemotePlatform) -> String {
    match p.os {
        RemoteOs::Windows => windows_command(
            r#"& (Join-Path $env:LOCALAPPDATA 'Neutrasearch\neutrasearch-helper.exe') --build"#,
        ),
        // Do not quote this path: the remote Unix shell must expand `~`.
        _ => format!("{} --build", remote_helper_path(p)),
    }
}

pub(super) fn connect_command(p: &RemotePlatform) -> String {
    match p.os {
        RemoteOs::Windows => windows_command(
            r#"& (Join-Path $env:LOCALAPPDATA 'Neutrasearch\neutrasearch-helper.exe')"#,
        ),
        _ => remote_helper_path(p).into(),
    }
}

pub(super) fn unix_install_command(expected_sha256: &str, os: RemoteOs) -> String {
    let hash_command = match os {
        RemoteOs::Linux => "sha256sum",
        RemoteOs::Macos => "shasum -a 256",
        RemoteOs::Windows => unreachable!("Windows uses a PowerShell install command"),
    };
    format!(
        "actual=$({hash_command} ~/.local/lib/neutrasearch/neutrasearch-helper.new | cut -d' ' -f1); test \"$actual\" = '{expected_sha256}' && chmod 700 ~/.local/lib/neutrasearch/neutrasearch-helper.new && mv ~/.local/lib/neutrasearch/neutrasearch-helper.new ~/.local/lib/neutrasearch/neutrasearch-helper"
    )
}

pub(super) fn windows_install_command(expected_sha256: &str) -> String {
    windows_command(&format!(
        "$source = Join-Path $HOME 'neutrasearch-helper.exe.new'; $actual = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant(); if ($actual -ne '{expected_sha256}') {{ Remove-Item -LiteralPath $source -Force; exit 42 }}; Move-Item -LiteralPath $source -Destination (Join-Path $env:LOCALAPPDATA 'Neutrasearch\\neutrasearch-helper.exe') -Force"
    ))
}
