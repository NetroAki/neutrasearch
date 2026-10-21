use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "native-cursor-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn roundtrip_requires_matching_generation_and_checksum() {
    let fixture = Fixture::new();
    let base = fixture.0.join("index.nsx");
    let cursors = vec![NativeCursor {
        mount: "/mnt/data".into(),
        tree: 257,
        since: 42,
    }];
    save_seed(&base, 91, &cursors).unwrap();
    assert_eq!(load_seed(&base, 91).unwrap(), cursors);
    assert_eq!(
        load_seed(&base, 92).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    let path = seed_path(&base);
    let mut bytes = fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(path, bytes).unwrap();
    assert_eq!(
        load_seed(&base, 91).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn rejects_zero_generation_and_duplicate_cursors() {
    let fixture = Fixture::new();
    let base = fixture.0.join("index.nsx");
    let cursor = NativeCursor {
        mount: "/mnt/data".into(),
        tree: 257,
        since: 42,
    };
    assert_eq!(
        save_seed(&base, 0, std::slice::from_ref(&cursor))
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        save_seed(&base, 1, &[cursor.clone(), cursor])
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
}
