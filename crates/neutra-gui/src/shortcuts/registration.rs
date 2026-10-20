use global_hotkey::hotkey::HotKey;

pub(super) fn update(
    active: &mut Vec<HotKey>,
    candidate: &[HotKey],
    mut register: impl FnMut(HotKey) -> Result<(), String>,
    mut unregister: impl FnMut(HotKey) -> Result<(), String>,
) -> Result<(), String> {
    let additions: Vec<_> = candidate
        .iter()
        .filter(|key| !active.contains(key))
        .copied()
        .collect();
    let removals: Vec<_> = active
        .iter()
        .filter(|key| !candidate.contains(key))
        .copied()
        .collect();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let result = (|| {
        for key in additions {
            register(key)?;
            added.push(key);
        }
        for key in removals {
            unregister(key)?;
            removed.push(key);
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut failures = vec![error];
        for key in added {
            if let Err(error) = unregister(key) {
                failures.push(error);
            }
        }
        for key in removed {
            if let Err(error) = register(key) {
                failures.push(error);
            }
        }
        return Err(format!(
            "Cannot change global shortcuts: {}",
            failures.join("; ")
        ));
    }
    *active = candidate.to_vec();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashSet};

    #[test]
    fn conflict_and_failed_removal_restore_the_previous_bindings() {
        let keys: Vec<HotKey> = ["Alt+K", "Alt+J", "Alt+L"]
            .iter()
            .map(|key| key.parse().unwrap())
            .collect();
        let mut active = vec![keys[0]];
        let installed = RefCell::new(HashSet::from([keys[0]]));
        let result = update(
            &mut active,
            &keys,
            |key| {
                if key == keys[2] {
                    return Err("another app owns Alt+L".into());
                }
                installed.borrow_mut().insert(key);
                Ok(())
            },
            |key| {
                installed.borrow_mut().remove(&key);
                Ok(())
            },
        );
        assert!(result.unwrap_err().contains("another app"));
        assert_eq!(active, vec![keys[0]]);
        assert_eq!(*installed.borrow(), HashSet::from([keys[0]]));

        active = keys[..2].to_vec();
        *installed.borrow_mut() = HashSet::from([keys[0], keys[1]]);
        let result = update(
            &mut active,
            &keys[2..],
            |key| {
                installed.borrow_mut().insert(key);
                Ok(())
            },
            |key| {
                if key == keys[1] {
                    return Err("failed to remove Alt+J".into());
                }
                installed.borrow_mut().remove(&key);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(active, keys[..2]);
        assert_eq!(*installed.borrow(), HashSet::from([keys[0], keys[1]]));
    }
}
