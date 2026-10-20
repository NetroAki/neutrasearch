use neutra_core::{FileKind, FileRecord, Index};

/// Dense screenshot-style fixture for view tests. Formerly the GUI
/// reference-mode dataset; reference mode itself is gone.
pub(super) fn dense_unicode_index() -> Index {
    let mut records = directory_records();
    records.extend(file_records());
    let folders = [
        "/home/alex/Documents/Accounts/2026",
        "/home/alex/Documents/Accounts/2025",
        "/mnt/studio/Clients/Invoices",
        "/home/alex/Downloads",
        "/home/alex/Work/Operations/Billing",
    ];
    let types = [
        ("pdf", 0_u64),
        ("pdf", 1),
        ("xlsx", 2),
        ("docx", 3),
        ("zip", 4),
        ("png", 5),
    ];
    for index in 0..48_u64 {
        let (extension, offset) = types[index as usize % types.len()];
        let magnitude = 180 * 1024 + ((index * 173 * 1024) % (9 * 1024 * 1024));
        records.push(record(
            format!(
                "{}/invoice-{}-{:02}-{:04}.{}",
                folders[index as usize % folders.len()],
                2026 - (index % 3),
                index % 12 + 1,
                index + 1042,
                extension
            ),
            magnitude,
            2 + index as i64 * 3,
            FileKind::File,
            if index % 5 == 2 {
                neutra_core::FsKind::Network("smb3".into())
            } else {
                neutra_core::FsKind::Ext4
            },
            100 + index + offset,
        ));
    }
    let mut index = Index::new();
    index.extend(records);
    index
}

fn record(
    path: String,
    size: u64,
    age_hours: i64,
    kind: FileKind,
    fs: neutra_core::FsKind,
    id: u64,
) -> FileRecord {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    FileRecord {
        path: path.into(),
        size,
        mtime: now.saturating_sub(age_hours * 3_600),
        mode: if kind == FileKind::Dir {
            0o040755
        } else {
            0o100644
        },
        kind,
        fs,
        native_id: id,
        native_parent: id.saturating_sub(1),
        source: 0,
        disk: size,
    }
}

fn directory_records() -> Vec<FileRecord> {
    vec![
        record(
            "/home/alex/Documents/Accounts".into(),
            0,
            3,
            FileKind::Dir,
            neutra_core::FsKind::Ext4,
            10,
        ),
        record(
            "/home/alex/Documents/Projects".into(),
            0,
            5,
            FileKind::Dir,
            neutra_core::FsKind::Ext4,
            11,
        ),
        record(
            "/home/alex/Documents".into(),
            0,
            4,
            FileKind::Dir,
            neutra_core::FsKind::Ext4,
            12,
        ),
        record(
            "/home/alex/Pictures".into(),
            0,
            30,
            FileKind::Dir,
            neutra_core::FsKind::Ext4,
            13,
        ),
        record(
            "/home/alex/Games".into(),
            0,
            40,
            FileKind::Dir,
            neutra_core::FsKind::Ext4,
            14,
        ),
    ]
}

fn file_records() -> Vec<FileRecord> {
    vec![
        record(
            "/home/alex/Downloads/invoice-final.pdf".into(),
            482 * 1024,
            1,
            FileKind::File,
            neutra_core::FsKind::Ext4,
            20,
        ),
        record(
            "/home/alex/Documents/Accounts/invoice-tracker.xlsx".into(),
            86 * 1024,
            4,
            FileKind::File,
            neutra_core::FsKind::Ext4,
            21,
        ),
        record(
            "/home/alex/Documents/Accounts/invoice-template.docx".into(),
            42 * 1024,
            9,
            FileKind::File,
            neutra_core::FsKind::Ext4,
            22,
        ),
        record(
            "/home/alex/Documents/Accounts/发票-上海-七月.pdf".into(),
            720 * 1024,
            12,
            FileKind::File,
            neutra_core::FsKind::Ext4,
            23,
        ),
        record(
            "/home/alex/Documents/Accounts/فاتورة-يوليو.pdf".into(),
            615 * 1024,
            16,
            FileKind::File,
            neutra_core::FsKind::Ext4,
            24,
        ),
        record(
            "/home/alex/Documents/Accounts/चालान-जुलाई.pdf".into(),
            530 * 1024,
            20,
            FileKind::File,
            neutra_core::FsKind::Ext4,
            25,
        ),
        record(
            "/home/alex/Documents/Projects/Aurora/site-plan.svg".into(),
            1_540 * 1024,
            25,
            FileKind::File,
            neutra_core::FsKind::Ext4,
            26,
        ),
        record(
            "/home/alex/Pictures/Library/photo-library.bin".into(),
            82_u64 << 30,
            32,
            FileKind::File,
            neutra_core::FsKind::Ext4,
            27,
        ),
        record(
            "/home/alex/Games/Orion/game-data.pak".into(),
            118_u64 << 30,
            40,
            FileKind::File,
            neutra_core::FsKind::Ext4,
            28,
        ),
        record(
            "/home/alex/Videos/family-video.mp4".into(),
            18_u64 << 30,
            48,
            FileKind::File,
            neutra_core::FsKind::Ext4,
            29,
        ),
        record(
            "/var/lib/containers/container-storage.bin".into(),
            39_u64 << 30,
            52,
            FileKind::File,
            neutra_core::FsKind::Ext4,
            30,
        ),
        record(
            "/usr/lib/runtime-libraries.bin".into(),
            27_u64 << 30,
            55,
            FileKind::File,
            neutra_core::FsKind::Ext4,
            31,
        ),
        record(
            "/mnt/studio/Media/camera-originals.bin".into(),
            164_u64 << 30,
            60,
            FileKind::File,
            neutra_core::FsKind::Network("smb3".into()),
            32,
        ),
    ]
}
