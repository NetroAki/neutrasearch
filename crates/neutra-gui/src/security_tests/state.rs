use crate::{load_gui_settings, save_gui_settings, GuiSettings};

#[test]
fn gui_settings_roundtrip_preserves_completed_onboarding_and_roots() {
    let directory =
        std::env::temp_dir().join(format!("neutrasearch-gui-settings-{}", std::process::id()));
    let path = directory.join("gui-settings.json");
    let _ = std::fs::remove_dir_all(&directory);
    let root = std::env::temp_dir();
    let settings = GuiSettings {
        onboarding_complete: true,
        roots: vec![root.clone()],
        ..Default::default()
    };
    save_gui_settings(&path, &settings).unwrap();
    let incomplete = GuiSettings {
        onboarding_complete: false,
        roots: vec![root.clone()],
        ..Default::default()
    };
    save_gui_settings(&path, &incomplete).unwrap();
    save_gui_settings(&path, &settings).unwrap();
    let loaded = load_gui_settings(&path).unwrap();
    assert!(loaded.onboarding_complete);
    assert_eq!(loaded.roots, vec![root]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}
