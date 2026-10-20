use super::*;

impl DeltaIndex {
    pub(crate) fn covers_catalog_cursor(
        &self,
        generation: &str,
        identity: &str,
        bytes: &str,
    ) -> bool {
        #[cfg(unix)]
        let own_identity = format!("{}:{}", self.identity.0, self.identity.1);
        #[cfg(not(unix))]
        let own_identity = String::new();
        generation == self.generation.to_string()
            && identity == own_identity
            && bytes
                .parse::<u64>()
                .is_ok_and(|bytes| bytes >= HEADER && bytes <= self.wal_bytes)
    }
}

pub(super) fn overlay_cursor(
    generation: u64,
    wal_bytes: u64,
    legacy_frames: bool,
    metadata: &Metadata,
) -> io::Result<OverlayCursor> {
    #[cfg(unix)]
    let (dev, ino) = file_identity_from_metadata(metadata)?;
    let modified_ns = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    Ok(OverlayCursor {
        generation,
        wal_bytes,
        legacy_frames,
        modified_ns,
        #[cfg(unix)]
        dev,
        #[cfg(unix)]
        ino,
    })
}

#[cfg(unix)]
pub(super) fn same_file_identity(a: OverlayCursor, b: OverlayCursor) -> bool {
    a.dev == b.dev && a.ino == b.ino
}
#[cfg(not(unix))]
pub(super) fn same_file_identity(_: OverlayCursor, _: OverlayCursor) -> bool {
    true
}
