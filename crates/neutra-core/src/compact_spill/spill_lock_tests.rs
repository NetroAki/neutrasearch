use super::*;

#[test]
fn concurrent_ingest_cannot_remove_an_active_spill() {
    let output = std::env::temp_dir().join(format!("neutra-spill-lock-{}.nsx", std::process::id()));
    let first = SpillAccumulator::begin(&output).unwrap();
    let sentinel = output.with_extension("spill").join("active.run");
    std::fs::write(&sentinel, b"active build").unwrap();
    assert!(
        matches!(SpillAccumulator::begin(&output), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"active build");
    let runs = first.finish().unwrap();
    assert!(
        matches!(SpillAccumulator::begin(&output), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
    drop(runs);
    drop(SpillAccumulator::begin(&output).unwrap());
    std::fs::remove_file(output.with_extension("spill.lock")).unwrap();
}
