use super::*;

fn settle(ops: &mut FileOperations) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while ops.pending != 0 {
        ops.update(&egui::Context::default());
        assert!(
            std::time::Instant::now() < deadline,
            "file operation did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn partial_batch_undo_keeps_one_retry_group_without_redo() {
    let mut ops = FileOperations::new(egui::Context::default());
    let tokens = vec![PathBuf::from("trash:///one"), PathBuf::from("trash:///two")];
    ops.undo.push(Undo::TrashMany(tokens.clone()));
    // Drive the same completion queue the worker uses, without touching
    // the user's Trash or depending on its desktop D-Bus session.
    let (tx, rx) = mpsc::channel();
    ops.rx = rx;
    ops.pending = 1;
    ops.undo_pending = true;
    tx.send(Completion::Batch {
        action: Action::RestoreMany(tokens.clone()),
        tokens: vec![tokens[1].clone()],
        error: Some("destination exists".into()),
    })
    .unwrap();
    ops.update(&egui::Context::default());
    assert!(!ops.undo_pending);
    assert_eq!(ops.pending, 0);
    assert!(
        matches!(&ops.undo[..], [Undo::TrashMany(remaining)] if remaining == &vec![tokens[1].clone()])
    );
    tx.send(Completion::Batch {
        action: Action::RestoreMany(vec![tokens[1].clone()]),
        tokens: vec![],
        error: None,
    })
    .unwrap();
    ops.update(&egui::Context::default());
    assert!(ops.undo.is_empty());
}

#[test]
fn rename_undo_keeps_failed_entries_and_never_becomes_redo() {
    let root = std::env::temp_dir().join(format!("neutra-undo-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let original = root.join("original");
    let renamed = root.join("renamed");
    let second = root.join("second");
    std::fs::write(&original, "preserve me").unwrap();
    let mut ops = FileOperations::new(egui::Context::default());
    ops.send(Action::Rename {
        from: original.clone(),
        to: renamed.clone(),
    });
    settle(&mut ops);
    ops.send(Action::Rename {
        from: renamed.clone(),
        to: second.clone(),
    });
    settle(&mut ops);
    ops.undo();
    settle(&mut ops);
    assert!(renamed.exists());
    assert!(!second.exists());
    std::fs::write(&original, "conflict").unwrap();
    ops.undo();
    settle(&mut ops);
    assert_eq!(ops.undo.len(), 1);
    assert_eq!(std::fs::read(&original).unwrap(), b"conflict");
    std::fs::remove_file(&original).unwrap();
    ops.undo();
    settle(&mut ops);
    assert_eq!(std::fs::read(&original).unwrap(), b"preserve me");
    assert!(ops.undo.is_empty());
    ops.undo();
    assert!(original.exists());
    assert!(!renamed.exists());
    ops.send(Action::Rename {
        from: original.clone(),
        to: renamed.clone(),
    });
    ops.undo();
    settle(&mut ops);
    assert!(
        original.exists(),
        "undo pressed during an operation must run after it completes"
    );
    assert!(!renamed.exists());
    std::fs::remove_dir_all(root).unwrap();
}
