use crate::records::{Alias, Entry};
use std::collections::HashMap;

pub(crate) fn merge(
    extensions: Vec<(u64, Entry, Vec<Alias>)>,
    entries: &mut HashMap<u64, Entry>,
    aliases: &mut HashMap<u64, Vec<Alias>>,
) {
    for (reference, entry, mut names) in extensions {
        let id = reference & 0x0000_ffff_ffff_ffff;
        let sequence = (reference >> 48) as u16;
        let Some(base) = entries
            .get_mut(&id)
            .filter(|base| sequence == 0 || base.sequence == sequence)
        else {
            continue;
        };
        base.size = base.size.max(entry.size);
        base.alloc = base.alloc.max(entry.alloc);
        names.push(Alias {
            parent: entry.parent,
            parent_sequence: entry.parent_sequence,
            name: entry.name,
        });
        let destination = aliases.entry(id).or_default();
        for alias in names {
            if !destination.contains(&alias)
                && !(alias.parent == base.parent && alias.name == base.name)
            {
                destination.push(alias);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_extensions_keep_names_and_reject_reused_base_records() {
        let base = Entry {
            parent: 5,
            parent_sequence: 0,
            sequence: 3,
            name: "base".into(),
            size: 1,
            alloc: 1,
            mtime: 0,
            dir: false,
        };
        let mut extension = base.clone();
        extension.name = "earlier-extension".into();
        extension.size = 9;
        let pending = vec![
            (10 | (3 << 48), extension.clone(), vec![]),
            (
                10 | (2 << 48),
                Entry {
                    name: "obsolete".into(),
                    ..extension
                },
                vec![],
            ),
        ];
        let mut entries = HashMap::from([(10, base)]);
        let mut aliases = HashMap::new();
        merge(pending, &mut entries, &mut aliases);
        assert_eq!(entries[&10].size, 9);
        assert_eq!(aliases[&10].len(), 1);
        assert_eq!(aliases[&10][0].name, "earlier-extension");
    }
}
