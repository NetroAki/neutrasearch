use super::*;

mod directories;
mod moves;
mod refresh;

#[cfg(test)]
mod tests;

pub(super) struct Plan<'a> {
    tree: &'a dyn Tree,
    base: &'a CompactIndex,
    delta: Option<&'a DeltaIndex>,
    mounted: &'a Mounted<'a>,
    browser: Option<&'a neutra_core::BrowserIndex>,
    inodes: HashMap<u64, &'a Inode>,
    prefixes: Vec<DeltaChange>,
    out: BTreeMap<String, DeltaChange>,
    added: Vec<(String, u64, Child)>,
    seen: HashSet<u64>,
    handled: HashSet<usize>,
}

impl<'a> Plan<'a> {
    pub(super) fn new(
        tree: &'a dyn Tree,
        changes: &'a Changes,
        base: &'a CompactIndex,
        delta: Option<&'a DeltaIndex>,
        mounted: &'a Mounted<'a>,
        browser: Option<&'a neutra_core::BrowserIndex>,
    ) -> Self {
        Self {
            tree,
            base,
            delta,
            mounted,
            browser,
            inodes: changes
                .inodes
                .iter()
                .map(|(inode, _)| (inode.ino, inode))
                .collect(),
            prefixes: Vec::new(),
            out: BTreeMap::new(),
            added: Vec::new(),
            seen: HashSet::new(),
            handled: HashSet::new(),
        }
    }

    pub(super) fn finish(mut self) -> Vec<DeltaChange> {
        self.prefixes.extend(self.out.into_values());
        self.prefixes
    }

    fn children(&self, path: &str) -> Result<Vec<FileRecord>> {
        match self.browser {
            Some(browser) => Ok(browser.directory_children(path)?),
            None => Ok(self.base.directory_children(path, self.delta)?),
        }
    }
}
