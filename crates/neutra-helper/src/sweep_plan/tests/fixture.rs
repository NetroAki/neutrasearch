use super::*;

#[derive(Default)]
pub(super) struct Fake {
    pub(super) child_reads: std::cell::Cell<usize>,
    pub(super) dirs: HashMap<u64, String>,
    pub(super) kids: HashMap<u64, Vec<Child>>,
    pub(super) inodes: HashMap<u64, Inode>,
    pub(super) refs: HashMap<u64, Vec<(u64, String)>>,
}

impl Tree for Fake {
    fn dir_path(&self, ino: u64) -> Result<Option<String>> {
        Ok(self.dirs.get(&ino).cloned())
    }
    fn children(&self, dir: u64) -> Result<Vec<Child>> {
        self.child_reads.set(self.child_reads.get() + 1);
        Ok(self.kids.get(&dir).cloned().unwrap_or_default())
    }
    fn inode(&self, ino: u64) -> Result<Option<Inode>> {
        Ok(self.inodes.get(&ino).cloned())
    }
    fn parent_refs(&self, ino: u64) -> Result<Vec<(u64, String)>> {
        Ok(self.refs.get(&ino).cloned().unwrap_or_default())
    }
}
