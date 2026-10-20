use super::*;

pub(super) fn normalize_arch(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" | "x64" => "x86_64".into(),
        "aarch64" | "arm64" => "aarch64".into(),
        x => x.into(),
    }
}
pub(super) fn validate_host(host: &str) -> Result<()> {
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
