//! Incremental Btrfs lane: which inodes and directories changed since a
//! transaction id. `TREE_SEARCH` with a minimum transid skips every leaf that
//! has not changed, so the cost follows the amount of change, not the size of
//! the filesystem. No directory is read and no file is stat'ed.

use super::*;
use std::collections::BTreeMap;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

mod parse;
use parse::{dir_changed, parse_dir_index, parse_inode, parse_ref, parse_refs, successor};

const DIR_INDEX: u32 = 96;
const ROOT_ITEM: u32 = 132;
const ROOT_TREE: u64 = 1;
const INO_LOOKUP_BYTES: usize = 4096;
const INO_LOOKUP: libc::c_ulong = (((IOC_READ | IOC_WRITE) << IOC_DIRSHIFT)
    | ((INO_LOOKUP_BYTES as u64) << IOC_SIZESHIFT)
    | (0x94 << IOC_TYPESHIFT)
    | (18 << IOC_NRSHIFT)) as libc::c_ulong;
/// A sweep buffer big enough for a few hundred leaves per call.
const BUF_U64S: usize = 128 * 1024;

type Key = (u64, u32, u64);

#[repr(C)]
struct InoLookupArgs {
    tree_id: u64,
    object_id: u64,
    name: [u8; INO_LOOKUP_BYTES - 16],
}

/// A changed inode's attributes.
#[derive(Clone, Debug)]
pub struct Inode {
    pub ino: u64,
    pub size: u64,
    pub disk: u64,
    pub mode: u32,
    pub mtime: i64,
    pub kind: FileKind,
}

/// One name inside a directory.
#[derive(Clone, Debug)]
pub struct Child {
    pub name: String,
    pub ino: u64,
    pub kind: FileKind,
}

/// What a sweep found. `generation` is the newest leaf generation seen; the
/// next sweep can start there.
#[derive(Debug, Default)]
pub struct Changes {
    pub generation: u64,
    /// Inodes whose own record changed, with their first parent link when it
    /// sat in a changed leaf too.
    pub inodes: Vec<(Inode, Option<(u64, String)>)>,
    /// Directories whose entry list changed (a name was added, removed or moved).
    pub dirs: Vec<u64>,
}

pub struct Subvolume {
    file: File,
    tree_id: u64,
}

impl Subvolume {
    /// Open the subvolume that contains `dir`.
    pub fn open(dir: &Path) -> Result<Self> {
        let file = File::open(dir).with_context(|| format!("open {}", dir.display()))?;
        let mut this = Self { file, tree_id: 0 };
        let (tree_id, _) = this.ino_lookup(ROOT_INO).context("BTRFS_IOC_INO_LOOKUP")?;
        this.tree_id = tree_id;
        Ok(this)
    }

    pub fn tree_id(&self) -> u64 {
        self.tree_id
    }

    /// Path, within this opened tree, of the directory this handle refers to.
    /// Opening the subvolume root yields an empty path; opening a bind-mounted
    /// subdirectory yields its path relative to that same Btrfs tree.
    pub fn opened_dir_path(&self) -> Result<String> {
        let ino = self.file.metadata()?.ino();
        self.dir_path(ino)?
            .context("opened Btrfs directory has no path")
    }

    /// The generation of this subvolume's last committed change.
    pub fn current_generation(&self) -> Result<u64> {
        let mut found = 0;
        let key = (self.tree_id, ROOT_ITEM, 0);
        self.search(
            ROOT_TREE,
            key,
            (self.tree_id, ROOT_ITEM, u64::MAX),
            0,
            &mut |item, data| {
                if item.item_type == ROOT_ITEM && data.len() >= 168 {
                    found = found.max(le64(&data[160..]));
                }
            },
        )?;
        if found == 0 {
            bail!("no root item for Btrfs subvolume {}", self.tree_id);
        }
        Ok(found)
    }

    /// Everything that changed in leaves written at or after `since`.
    pub fn changes_since(&self, since: u64) -> Result<Changes> {
        let mut generation = since;
        let mut inodes = BTreeMap::<u64, Inode>::new();
        let mut dirs = std::collections::BTreeSet::<u64>::new();
        self.search(
            self.tree_id,
            (0, 0, 0),
            (u64::MAX, u32::MAX, u64::MAX),
            since,
            &mut |item, data| {
                generation = generation.max(item.transid);
                match item.item_type {
                    INODE_ITEM if data.len() >= 160 && le64(&data[8..]) >= since => {
                        inodes.insert(item.objectid, parse_inode(item.objectid, data));
                    }
                    DIR_INDEX if dir_changed(data, since) => {
                        dirs.insert(item.objectid);
                    }
                    _ => {}
                }
            },
        )?;
        let inodes = inodes.into_values().map(|inode| (inode, None)).collect();
        Ok(Changes {
            generation,
            inodes,
            dirs: dirs.into_iter().collect(),
        })
    }

    /// Path of directory `ino` below the subvolume root without outer
    /// slashes ("" for the root), or `None` when it no longer exists.
    pub fn dir_path(&self, ino: u64) -> Result<Option<String>> {
        match self.ino_lookup(ino) {
            Ok((_, path)) => Ok(Some(path)),
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(None),
            Err(error) => Err(error).context("BTRFS_IOC_INO_LOOKUP"),
        }
    }

    /// The names inside directory `dir`, as they are now.
    pub fn children(&self, dir: u64) -> Result<Vec<Child>> {
        let mut out = Vec::new();
        self.search(
            self.tree_id,
            (dir, DIR_INDEX, 0),
            (dir, DIR_INDEX, u64::MAX),
            0,
            &mut |item, data| {
                if item.item_type == DIR_INDEX {
                    out.extend(parse_dir_index(data));
                }
            },
        )?;
        Ok(out)
    }

    pub fn inode(&self, ino: u64) -> Result<Option<Inode>> {
        let mut found = None;
        self.search(
            self.tree_id,
            (ino, INODE_ITEM, 0),
            (ino, INODE_ITEM, 0),
            0,
            &mut |item, data| {
                if item.item_type == INODE_ITEM && data.len() >= 160 {
                    found = Some(parse_inode(item.objectid, data));
                }
            },
        )?;
        Ok(found)
    }

    /// First parent link of `ino`.
    pub fn parent_ref(&self, ino: u64) -> Result<Option<(u64, String)>> {
        let mut found = None;
        self.search(
            self.tree_id,
            (ino, INODE_REF, 0),
            (ino, INODE_REF, u64::MAX),
            0,
            &mut |item, data| {
                if found.is_none() && item.item_type == INODE_REF {
                    found = parse_ref(item.offset, data);
                }
            },
        )?;
        Ok(found)
    }

    /// Every parent/name link of `ino`, including extended references.
    pub fn parent_refs(&self, ino: u64) -> Result<Vec<(u64, String)>> {
        let mut found = Vec::new();
        self.search(
            self.tree_id,
            (ino, INODE_REF, 0),
            (ino, INODE_EXTREF, u64::MAX),
            0,
            &mut |item, data| match item.item_type {
                INODE_REF => {
                    found.extend(parse_refs(item.offset, data, false));
                }
                INODE_EXTREF => {
                    found.extend(parse_refs(0, data, true));
                }
                _ => {}
            },
        )?;
        Ok(found)
    }

    fn ino_lookup(&self, ino: u64) -> std::io::Result<(u64, String)> {
        // SAFETY: all-zero is a valid InoLookupArgs.
        let mut args: InoLookupArgs = unsafe { std::mem::zeroed() };
        args.tree_id = if self.tree_id == 0 { 0 } else { self.tree_id };
        args.object_id = ino;
        // SAFETY: args is a live, correctly sized request buffer for this ioctl.
        let rc = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                INO_LOOKUP,
                &mut args as *mut InoLookupArgs,
            )
        };
        if rc < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let end = args
            .name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(args.name.len());
        let path = String::from_utf8_lossy(&args.name[..end])
            .trim_matches('/')
            .to_string();
        Ok((args.tree_id, path))
    }

    /// Visit every item in the key range written at or after `min_transid`.
    fn search(
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

struct SweepItem {
    transid: u64,
    objectid: u64,
    offset: u64,
    item_type: u32,
}
