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
