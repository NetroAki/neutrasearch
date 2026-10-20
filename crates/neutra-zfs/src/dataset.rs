use anyhow::{bail, Result};

pub(super) fn validate(dataset: &str) -> Result<()> {
    if dataset.is_empty()
        || dataset.len() >= 256
        || dataset.starts_with('-')
        || dataset
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !dataset
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.:/".contains(&byte))
    {
        bail!("Invalid ZFS filesystem dataset name");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate;

    #[test]
    fn only_filesystem_dataset_names_are_accepted() {
        for dataset in ["tank", "tank/music", "tank/nvme-1.backup"] {
            assert!(validate(dataset).is_ok());
        }
        for dataset in [
            "",
            "-r",
            "/tank",
            "tank//data",
            "tank/../data",
            "tank/.",
            "tank@other",
            "tank/a b",
            "tank\0",
        ] {
            assert!(validate(dataset).is_err(), "{dataset:?}");
        }
    }
}
