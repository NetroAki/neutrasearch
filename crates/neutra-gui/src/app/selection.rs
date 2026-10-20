use std::{collections::BTreeSet, path::PathBuf};

#[derive(Default)]
pub(crate) struct Selection {
    paths: BTreeSet<String>,
    anchor: Option<String>,
}

impl Selection {
    pub(crate) fn contains(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    pub(crate) fn len(&self) -> usize {
        self.paths.len()
    }

    pub(crate) fn paths(&self) -> Vec<PathBuf> {
        self.paths.iter().map(PathBuf::from).collect()
    }

    pub(crate) fn single(&mut self, path: &str) {
        self.paths.clear();
        self.paths.insert(path.into());
        self.anchor = Some(path.into());
    }

    pub(crate) fn click(&mut self, path: &str, toggle: bool, range: bool, ordered: &[&str]) {
        if range {
            let start = self
                .anchor
                .as_deref()
                .and_then(|a| ordered.iter().position(|p| *p == a));
            let end = ordered.iter().position(|p| *p == path);
            if let (Some(start), Some(end)) = (start, end) {
                if !toggle {
                    self.paths.clear();
                }
                self.paths.extend(
                    ordered[start.min(end)..=start.max(end)]
                        .iter()
                        .map(|p| (*p).into()),
                );
                return;
            }
        }
        if toggle {
            if !self.paths.remove(path) {
                self.paths.insert(path.into());
            }
            self.anchor = Some(path.into());
        } else {
            self.single(path);
        }
    }

    pub(crate) fn all(&mut self, ordered: &[&str]) {
        self.paths = ordered.iter().map(|p| (*p).into()).collect();
        self.anchor = ordered.first().map(|p| (*p).into());
    }

    pub(crate) fn retain(&mut self, ordered: &[&str]) {
        let visible: BTreeSet<_> = ordered.iter().copied().collect();
        self.paths.retain(|path| visible.contains(path.as_str()));
        if self
            .anchor
            .as_deref()
            .is_some_and(|path| !visible.contains(path))
        {
            self.anchor = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_toggle_and_shift_ranges_follow_display_order() {
        let order = ["z", "b", "a", "d"];
        let mut selected = Selection::default();
        selected.click("b", false, false, &order);
        selected.click("d", false, true, &order);
        assert_eq!(selected.paths(), ["a", "b", "d"].map(PathBuf::from));
        selected.click("a", true, false, &order);
        assert!(!selected.contains("a"));
        selected.click("z", true, true, &order);
        assert_eq!(selected.len(), 4);
        selected.click("b", false, false, &order);
        assert_eq!(selected.paths(), [PathBuf::from("b")]);
    }

    #[test]
    fn stale_and_unloaded_rows_cannot_remain_selected() {
        let mut selected = Selection::default();
        selected.all(&["a", "b", "c"]);
        selected.retain(&["c", "d"]);
        assert_eq!(selected.paths(), [PathBuf::from("c")]);
        selected.click("d", false, true, &["c", "d"]);
        assert_eq!(selected.paths(), [PathBuf::from("d")]);
    }
}
