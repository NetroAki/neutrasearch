use super::*;

impl Subvolume {
    /// Visit every item in the key range written at or after `min_transid`.
    pub(super) fn search(
        &self,
        tree_id: u64,
        min: Key,
        max: Key,
        min_transid: u64,
        visit: &mut dyn FnMut(&SweepItem, &[u8]),
    ) -> Result<()> {
        // Single-inode lookups need at most one leaf per call. Zeroing a
        // megabyte for every parent/ref lookup dominated busy reconciliations.
        let buf_u64s = if min.0 == max.0 { 8 * 1024 } else { BUF_U64S };
        let mut storage = vec![0u64; SEARCH_V2_HEADER / 8 + buf_u64s];
        // SAFETY: storage is 8-byte aligned and large enough for the header and buffer.
        let args = unsafe { &mut *(storage.as_mut_ptr().cast::<SearchArgsV2>()) };
        let mut cursor = min;
        loop {
            args.key = SearchKey {
                tree_id,
                min_objectid: cursor.0,
                max_objectid: max.0,
                min_offset: cursor.2,
                max_offset: max.2,
                min_transid,
                max_transid: u64::MAX,
                min_type: cursor.1,
                max_type: max.1,
                nr_items: 4096,
                ..SearchKey::default()
            };
            args.buf_size = (buf_u64s * 8) as u64;
            // SAFETY: args points at a live request followed by its result buffer.
            let rc = unsafe {
                libc::ioctl(
                    self.file.as_raw_fd(),
                    TREE_SEARCH_V2,
                    args as *mut SearchArgsV2,
                )
            };
            if rc < 0 {
                let error = std::io::Error::last_os_error();
                if matches!(error.raw_os_error(), Some(libc::EPERM) | Some(libc::EACCES)) {
                    bail!("BTRFS_IOC_TREE_SEARCH denied; the watcher needs CAP_SYS_ADMIN");
                }
                return Err(error).context("BTRFS_IOC_TREE_SEARCH");
            }
            let count = args.key.nr_items as usize;
            if count == 0 {
                return Ok(());
            }
            // SAFETY: the kernel filled up to buf_size bytes after the header.
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    (args as *const SearchArgsV2)
                        .cast::<u8>()
                        .add(SEARCH_V2_HEADER),
                    buf_u64s * 8,
                )
            };
            let mut pos = 0usize;
            let mut last = cursor;
            for _ in 0..count {
                if pos + 32 > bytes.len() {
                    bail!("kernel returned a truncated Btrfs search header");
                }
                let item = SweepItem {
                    transid: le64(&bytes[pos..]),
                    objectid: le64(&bytes[pos + 8..]),
                    offset: le64(&bytes[pos + 16..]),
                    item_type: le32(&bytes[pos + 24..]),
                };
                let len = le32(&bytes[pos + 28..]) as usize;
                pos += 32;
                let end = pos
                    .checked_add(len)
                    .filter(|end| *end <= bytes.len())
                    .context("truncated Btrfs search item")?;
                visit(&item, &bytes[pos..end]);
                pos = end;
                last = (item.objectid, item.item_type, item.offset);
            }
            let Some(next) = successor(last) else {
                return Ok(());
            };
            if next > max {
                return Ok(());
            }
            if next <= cursor {
                bail!("Btrfs search cursor failed to advance: {cursor:?} -> {next:?}");
            }
            cursor = next;
        }
    }
}
