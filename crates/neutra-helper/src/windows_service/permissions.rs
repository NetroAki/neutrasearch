use anyhow::{Context, Result};

pub(super) fn pipe_acl(mut arguments: impl Iterator<Item = String>) -> Result<String> {
    anyhow::ensure!(
        arguments.next().as_deref() == Some("--authorized-user"),
        "scanner service has no approved account; repair the installation"
    );
    let sid = arguments.next().context("missing approved account SID")?;
    anyhow::ensure!(arguments.next().is_none(), "unexpected service arguments");
    let components: Vec<_> = sid.split('-').collect();
    anyhow::ensure!(
        components.len() >= 5
            && components.len() <= 18
            && components[0] == "S"
            && components[1] == "1"
            && components[2..].iter().all(|part| {
                !part.is_empty()
                    && part.bytes().all(|byte| byte.is_ascii_digit())
                    && part.parse::<u32>().is_ok()
            }),
        "invalid approved account SID"
    );
    // Omit FILE_CREATE_PIPE_INSTANCE (0x4), which GENERIC_WRITE would grant.
    Ok(format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;{sid})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_requires_an_explicit_account() {
        assert!(pipe_acl(std::iter::empty()).is_err());
        for sid in ["AU", "WD", "S-1-1-0", "S-1-5-21-1)(A;;GA;;;WD)", "S-1-5--1"] {
            assert!(pipe_acl(["--authorized-user".into(), sid.into()].into_iter()).is_err());
        }
    }

    #[test]
    fn approved_account_gets_no_server_creation_rights() {
        let sid = "S-1-5-21-1-2-3-1001";
        let acl = pipe_acl(["--authorized-user".into(), sid.into()].into_iter()).unwrap();
        assert_eq!(
            acl,
            format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;{sid})")
        );
        assert!(
            pipe_acl(["--authorized-user".into(), sid.into(), "extra".into()].into_iter()).is_err()
        );
    }
}
