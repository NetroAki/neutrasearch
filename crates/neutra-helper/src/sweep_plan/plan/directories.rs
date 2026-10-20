use super::*;

impl Plan<'_> {
    pub(in crate::sweep_plan) fn discover(&mut self, changes: &Changes) -> Result<Vec<FileRecord>> {
        let mut removed = Vec::new();
        let mut queue = VecDeque::new();
        // Removing a folder's final entry changes its inode, even with no entry items left.
        for dir in changes.dirs.iter().copied().chain(
            changes
                .inodes
                .iter()
                .filter(|(i, _)| i.kind == FileKind::Dir)
                .map(|(i, _)| i.ino),
        ) {
            if self.seen.insert(dir) {
                queue.push_back(dir);
            }
        }
        while let Some(dir) = queue.pop_front() {
            let Some(relative) = self.tree.dir_path(dir)? else {
                continue;
            };
            let Some(relative) = mount_relative(self.mounted.relative_root, &relative) else {
                continue;
            };
            let path = full_path(self.mounted.prefix, &relative);
            let now = self.tree.children(dir)?;
            let known = self.children(&path)?;
            let now_names: HashSet<_> = now.iter().map(|child| child.name.as_str()).collect();
            let known_names: HashSet<_> = known
                .iter()
                .map(|r| r.path.rsplit('/').next().unwrap_or(""))
                .collect();
            removed.extend(
                known
                    .iter()
                    .filter(|r| !now_names.contains(r.path.rsplit('/').next().unwrap_or("")))
                    .cloned(),
            );
            for child in now
                .iter()
                .filter(|c| c.ino != 0 && !known_names.contains(c.name.as_str()))
            {
                self.added.push((path.clone(), dir, child.clone()));
            }
        }
        Ok(removed)
    }

    pub(in crate::sweep_plan) fn additions(&mut self) -> Result<()> {
        let mut position = 0;
        while position < self.added.len() {
            if self.handled.contains(&position) {
                position += 1;
                continue;
            }
            let (parent_path, parent, child) = self.added[position].clone();
            let path = child_path(&parent_path, &child.name);
            if let Some(inode) = inode_of(self.tree, &mut self.inodes, child.ino)? {
                self.out.insert(
                    path.clone(),
                    DeltaChange::Upsert(record(path.clone(), &inode, parent, self.mounted)),
                );
            }
            if child.kind == FileKind::Dir && self.seen.insert(child.ino) {
                let now = self.tree.children(child.ino)?;
                let known = self.children(&path)?;
                let known_names: HashSet<_> = known
                    .iter()
                    .map(|r| r.path.rsplit('/').next().unwrap_or(""))
                    .collect();
                for below in now
                    .into_iter()
                    .filter(|c| c.ino != 0 && !known_names.contains(c.name.as_str()))
                {
                    self.added.push((path.clone(), child.ino, below));
                }
            }
            position += 1;
        }
        Ok(())
    }
}
