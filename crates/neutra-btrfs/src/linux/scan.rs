use super::*;

pub fn scan(mount: &MountInfo, sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    let started = Instant::now();
    let (nodes, names, batches) = parallel::scan_metadata(&mount.mountpoint)?;
    let mut stats = ScanStats::default();
    let prefix = mount
        .mountpoint
        .to_string_lossy()
        .trim_end_matches('/')
        .to_string();
    let mut dir_paths = HashMap::<u64, String>::new();
    dir_paths.insert(ROOT_INO, String::new());
    for node in &nodes {
        let meta = node.meta;
        let kind = kind_from_mode(meta.mode);
        let path = if node.ino == ROOT_INO {
            if prefix.is_empty() {
                "/".to_string()
            } else {
                prefix.clone()
            }
        } else {
            let parent = node.parent;
            if parent == u64::MAX || !ensure_dir_path(parent, &nodes, &names, &mut dir_paths)? {
                continue;
            }
            let parent_path = dir_paths.get(&parent).unwrap();
            let name =
                node_name(node, &names).context("Btrfs filename arena contains invalid UTF-8")?;
            if kind == FileKind::Dir {
                let relative = append_component(parent_path, name);
                let full = prefix_path(&prefix, &relative);
                dir_paths.insert(node.ino, relative);
                full
            } else {
                let mut full =
                    String::with_capacity(prefix.len() + parent_path.len() + name.len() + 2);
                full.push_str(&prefix);
                full.push('/');
                if !parent_path.is_empty() {
                    full.push_str(parent_path);
                    full.push('/');
                }
                full.push_str(name);
                full
            }
        };
        if kind == FileKind::Dir {
            stats.dirs += 1
        } else {
            stats.files += 1
        };
        sink(FileRecord {
            path: path.into_boxed_str(),
            size: meta.size,
            disk: FileRecord::allocated_bytes(meta.disk),
            mtime: meta.mtime,
            mode: meta.mode,
            kind,
            fs: FsKind::Btrfs,
            native_id: node.ino,
            native_parent: if node.parent == u64::MAX {
                0
            } else {
                node.parent
            },
            source: 0,
        });
        stats.records += 1;
    }
    stats.wall_ms = started.elapsed().as_millis() as u64;
    stats.detail = format!(
        "TREE_SEARCH only; {} batches; mounted subvolume tree",
        batches
    );
    Ok(stats)
}
