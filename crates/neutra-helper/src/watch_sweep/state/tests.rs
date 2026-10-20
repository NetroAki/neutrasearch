use super::*;

struct Fixture(PathBuf);

impl Fixture {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("neutra-cursor-{label}-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn failed_cursor_replacement_keeps_the_previous_checkpoint() {
    let fixture = Fixture::new("failed-save");
    let state = fixture.0.join("index.sweep");
    let previous = "257 123 /home\n";
    std::fs::write(&state, previous).unwrap();
    std::fs::create_dir(append_suffix(&state, ".tmp")).unwrap();
    assert!(save(&[], &state).is_err());
    assert_eq!(std::fs::read_to_string(&state).unwrap(), previous);
}

#[test]
fn malformed_cursor_is_not_treated_as_current_metadata() {
    let fixture = Fixture::new("invalid");
    let state = fixture.0.join("index.sweep");
    for invalid in ["257 broken /home\n", "0 123 /home\n", "257 0 /home\n"] {
        std::fs::write(&state, invalid).unwrap();
        assert!(restore(&mut [], &state).is_err());
    }
    std::fs::write(&state, "257 123 /home/path with spaces\n").unwrap();
    restore(&mut [], &state).unwrap();
}

#[test]
fn saved_cursor_is_private_and_renamed_into_place() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("permissions");
    let state = fixture.0.join("index.sweep");
    save(&[], &state).unwrap();
    assert_eq!(
        std::fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!append_suffix(&state, ".tmp").exists());
}
