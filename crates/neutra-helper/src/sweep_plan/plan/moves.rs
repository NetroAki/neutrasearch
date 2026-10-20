use super::*;

impl Plan<'_> {
    pub(in crate::sweep_plan) fn moves(&mut self, removed: Vec<FileRecord>) -> Result<()> {
        let moved: HashMap<_, _> = self
            .added
            .iter()
            .enumerate()
            .map(|(p, (_, _, c))| (c.ino, p))
            .collect();
        for gone in removed {
            if matches!(gone.path.rsplit('/').next(), Some("." | ".." | "")) {
                // Remove obsolete pseudo-entries exactly; they must never become subtree tombstones.
                self.out
                    .insert(gone.path.to_string(), DeltaChange::Remove(gone.path));
                continue;
            }
            let target = (gone.fs == *self.mounted.fs && gone.source == self.mounted.source)
                .then(|| moved.get(&gone.native_id).copied())
                .flatten()
                .filter(|position| self.added[*position].2.kind == gone.kind);
            let subtree = self.subtree_for_move(&gone, target.is_some())?;
            if gone.kind == FileKind::Dir && target.is_none() {
                self.prefixes
                    .push(DeltaChange::RemovePrefix(gone.path.clone()));
            } else {
                self.out.insert(
                    gone.path.to_string(),
                    DeltaChange::Remove(gone.path.clone()),
                );
            }
            for below in &subtree {
                self.out.insert(
                    below.path.to_string(),
                    DeltaChange::Remove(below.path.clone()),
                );
            }
            if let Some(position) = target {
                self.relocate(position, &gone.path, subtree)?;
            }
        }
        Ok(())
    }

    fn subtree_for_move(&self, gone: &FileRecord, moving: bool) -> Result<Vec<FileRecord>> {
        if gone.kind != FileKind::Dir || !moving {
            return Ok(Vec::new());
        }
        let found = match self.browser {
            Some(browser) => browser.subtree_records(&gone.path, SUBTREE_LIMIT)?,
            None => self
                .base
                .subtree_records(&gone.path, self.delta, SUBTREE_LIMIT)?,
        };
        found.ok_or_else(|| anyhow::anyhow!("cannot reconcile folder {}: more than {SUBTREE_LIMIT} descendants require native prefix updates; the cursor was not advanced", gone.path))
    }

    fn relocate(
        &mut self,
        position: usize,
        old_path: &str,
        subtree: Vec<FileRecord>,
    ) -> Result<()> {
        let (dir_path, parent, child) = &self.added[position];
        let new_path = child_path(dir_path, &child.name);
        if let Some(inode) = inode_of(self.tree, &mut self.inodes, child.ino)? {
            self.out.insert(
                new_path.clone(),
                DeltaChange::Upsert(record(new_path.clone(), &inode, *parent, self.mounted)),
            );
        }
        for mut below in subtree {
            let path = format!("{new_path}{}", &below.path[old_path.len()..]);
            below.path = path.clone().into_boxed_str();
            self.out.insert(path, DeltaChange::Upsert(below));
        }
        self.handled.insert(position);
        Ok(())
    }
}
