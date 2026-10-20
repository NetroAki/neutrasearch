use super::*;
use std::path::Path;

fn path(base: &Path) -> std::path::PathBuf {
    append_suffix(base, ".compact-tail")
}

pub(in crate::store) fn write(
    base: &Path,
    previous: u64,
    generation: u64,
    bytes: u64,
) -> Result<()> {
    let destination = path(base);
    anyhow::ensure!(
        !destination.exists(),
        "unfinished tail publication requires recovery"
    );
    let temporary = append_suffix(&destination, ".new");
    match std::fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    file.write_all(b"NSTAIL01")?;
    for value in [previous, generation, bytes, u64::from(std::process::id())] {
        file.write_all(&value.to_le_bytes())?;
    }
    file.sync_all()?;
    drop(file);
    std::fs::rename(temporary, &destination)?;
    sync_parent(&destination)
}

fn read(base: &Path) -> Result<Option<[u64; 4]>> {
    let mut file = match std::fs::File::open(path(base)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        file.metadata()?.len() == 40,
        "invalid compaction tail intent length"
    );
    let mut bytes = [0; 40];
    file.read_exact(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() == 40 && &bytes[..8] == b"NSTAIL01",
        "invalid compaction tail intent"
    );
    let mut values = [0; 4];
    for (index, value) in values.iter_mut().enumerate() {
        let start = 8 + index * 8;
        *value = u64::from_le_bytes(bytes[start..start + 8].try_into().unwrap());
    }
    anyhow::ensure!(
        values[0] != 0
            && values[1] != 0
            && values[0] != values[1]
            && values[2] >= DELTA_HEADER_BYTES
            && values[3] != 0
            && values[3] <= u64::from(u32::MAX),
        "invalid compaction tail values"
    );
    Ok(Some(values))
}

pub(in crate::store) fn check(base: &Path, delta: &Path) -> Result<bool> {
    let Some([previous, generation, minimum, _]) = read(base)? else {
        return Ok(false);
    };
    let base_generation = CompactIndex::generation_on_disk(base)?;
    let mut file = std::fs::File::open(delta).context("open committed compaction tail")?;
    let mut header = [0; 16];
    file.read_exact(&mut header)
        .context("committed compaction tail header is incomplete")?;
    anyhow::ensure!(
        &header[..8] == b"NEUTDLT4",
        "invalid committed compaction tail format"
    );
    let wal_generation = u64::from_le_bytes(header[8..].try_into().unwrap());
    if wal_generation == previous {
        anyhow::ensure!(
            base_generation == previous && !compaction_marker(base).exists(),
            "compaction tail and published base disagree"
        );
    } else {
        anyhow::ensure!(
            wal_generation == generation
                && (base_generation == previous || base_generation == generation),
            "compaction tail generation mismatch"
        );
        anyhow::ensure!(
            file.metadata()?.len() >= minimum,
            "compaction tail is shorter than its committed length; refusing to discard updates"
        );
    }
    Ok(true)
}

pub(in crate::store) fn finish(base: &Path, delta: &Path) -> Result<()> {
    let Some([_, generation, _, pid]) = read(base)? else {
        return Ok(());
    };
    let temporary = delta
        .parent()
        .context("delta has no parent")?
        .join(".delta.overlay")
        .join(format!("fold-tail-{pid}-{generation}"));
    match std::fs::remove_file(temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    std::fs::remove_file(path(base))?;
    sync_parent(base)
}
