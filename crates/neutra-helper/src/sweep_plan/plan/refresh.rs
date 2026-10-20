use super::*;

impl Plan<'_> {
    pub(in crate::sweep_plan) fn refresh(&mut self, changes: &Changes) -> Result<()> {
        let mut parents = HashMap::<u64, Option<String>>::new();
        for (inode, link) in &changes.inodes {
            let Some(inode) = self.inodes.get(&inode.ino).copied() else {
                continue;
            };
            let mut links = self.tree.parent_refs(inode.ino)?;
            if let Some(link) = link {
                links.push(link.clone());
            }
            links.sort();
            links.dedup();
            for (parent, name) in links {
                if matches!(name.as_str(), "." | "..") {
                    continue;
                }
                if let std::collections::hash_map::Entry::Vacant(e) = parents.entry(parent) {
                    e.insert(
                        self.tree
                            .dir_path(parent)?
                            .and_then(|r| mount_relative(self.mounted.relative_root, &r))
                            .map(|r| full_path(self.mounted.prefix, &r)),
                    );
                }
                let Some(path) = parents.get(&parent).cloned().flatten() else {
                    continue;
                };
                let path = child_path(&path, &name);
                if self.out.contains_key(&path) {
                    continue;
                }
                let fresh = record(path.clone(), inode, parent, self.mounted);
                if !indexed(self.base, self.delta, self.browser, &path)?
                    .is_some_and(|known| same_content(&known, &fresh))
                {
                    self.out.insert(path, DeltaChange::Upsert(fresh));
                }
            }
        }
        Ok(())
    }
}
