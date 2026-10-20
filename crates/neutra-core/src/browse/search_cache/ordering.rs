use crate::{FileRecord, SortKey};

pub(super) fn key(sort: SortKey, score: u32, record: &FileRecord) -> Vec<u8> {
    let mut key = Vec::with_capacity(record.path.len() + record.name().len() + 16);
    let descending = matches!(
        sort,
        SortKey::Relevance
            | SortKey::NameDesc
            | SortKey::PathDesc
            | SortKey::SizeDesc
            | SortKey::MtimeDesc
    );
    match sort {
        SortKey::Relevance => {
            key.extend_from_slice(&(!score).to_be_bytes());
            timestamp(&mut key, record, true);
        }
        SortKey::MtimeAsc | SortKey::MtimeDesc => timestamp(&mut key, record, descending),
        SortKey::SizeAsc | SortKey::SizeDesc => {
            let value = record.disk_bytes();
            key.extend_from_slice(&(if descending { !value } else { value }).to_be_bytes());
        }
        SortKey::NameAsc | SortKey::NameDesc => {
            string(
                &mut key,
                record.name().bytes().map(|b| b.to_ascii_lowercase()),
                descending,
            );
        }
        SortKey::PathAsc | SortKey::PathDesc => {}
    }
    string(
        &mut key,
        record.path.bytes().map(crate::compact::fold_byte),
        descending,
    );
    key
}

fn timestamp(key: &mut Vec<u8>, record: &FileRecord, descending: bool) {
    let time = if record.mtime > 4_102_444_800 {
        0
    } else {
        record.mtime
    };
    let value = (time as u64) ^ (1 << 63);
    key.extend_from_slice(&(if descending { !value } else { value }).to_be_bytes());
}

fn string(key: &mut Vec<u8>, bytes: impl Iterator<Item = u8>, descending: bool) {
    // Two-byte units reserve a terminator even for embedded NULs.
    for byte in bytes {
        let unit = if descending { [254, !byte] } else { [1, byte] };
        key.extend_from_slice(&unit);
    }
    key.push(if descending { 255 } else { 0 });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileKind, FsKind};

    #[test]
    fn byte_keys_match_every_engine_order() {
        let paths = [
            "/a", "/a/a", "/A/a", "/a/ab", "/z/É", "/z/é", "/z/😀", "/nul\0x",
        ];
        let records: Vec<_> = paths
            .iter()
            .enumerate()
            .map(|(n, path)| FileRecord {
                path: (*path).into(),
                size: n as u64,
                disk: [0, u64::MAX, 20][n % 3],
                mtime: [i64::MIN, -1, 0, 3, 4_102_444_801][n % 5],
                mode: 0,
                kind: FileKind::File,
                fs: FsKind::Btrfs,
                native_id: 0,
                native_parent: 0,
                source: 0,
            })
            .collect();
        for sort in [
            SortKey::Relevance,
            SortKey::NameAsc,
            SortKey::NameDesc,
            SortKey::PathAsc,
            SortKey::PathDesc,
            SortKey::SizeAsc,
            SortKey::SizeDesc,
            SortKey::MtimeAsc,
            SortKey::MtimeDesc,
        ] {
            for (i, a) in records.iter().enumerate() {
                for (j, b) in records.iter().enumerate() {
                    let expected = crate::matcher::compare_records(
                        sort,
                        &(i as u32 % 3, a),
                        &(j as u32 % 3, b),
                    );
                    assert_eq!(
                        key(sort, i as u32 % 3, a).cmp(&key(sort, j as u32 % 3, b)),
                        expected,
                        "{sort:?}: {} / {}",
                        a.path,
                        b.path
                    );
                }
            }
        }
    }
}
