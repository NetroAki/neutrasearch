use super::*;

#[test]
fn unicode_needles_fold_without_pre_lowercasing() {
    // Regression: uppercase non-ASCII needles previously never matched
    // because only the haystack was folded.
    // Matches return (byte offset, byte length); non-ASCII letters span
    // multiple bytes in UTF-8.
    assert_eq!(
        find_term("café.md", "CAFÉ", "café", false, false, false),
        Some((0, 5))
    );
    assert_eq!(
        find_term("ärger.txt", "ÄRGER", "ärger", false, false, false),
        Some((0, 6))
    );
    assert_eq!(
        find_term("文件.txt", "文件", "文件", false, false, false),
        Some((0, 6))
    );
    assert_eq!(
        find_term("report.pdf", "MISSING", "missing", false, false, false),
        None
    );
    // `to_lowercase` is not full case folding: ß does not match "SS".
    // Same semantics as before this refactor; documented, not accidental.
    assert_eq!(
        find_term("Straße.txt", "STRASSE", "strasse", false, false, false),
        None
    );
}

#[test]
fn ascii_paths_stay_on_the_fast_byte_path() {
    assert_eq!(
        find_term("/a/Report.PDF", "report", "report", false, false, false),
        Some((3, 6))
    );
}

#[test]
fn whole_word_rejects_substrings() {
    let folded = "call".to_string();
    assert_eq!(
        find_term("the calling.wav", "call", &folded, false, true, false),
        None
    );
    assert_eq!(
        find_term("the call.wav", "call", &folded, false, true, false),
        Some((4, 4))
    );
    assert_eq!(
        find_term("recall.wav", "call", &folded, false, false, false),
        Some((2, 4))
    );
}

#[test]
fn accent_folding_is_opt_in() {
    let strict = "cafe".to_string();
    assert_eq!(
        find_term("caf\u{e9}.wav", "cafe", &strict, false, false, false),
        None
    );
    let folded = strip_accents_str("cafe");
    assert_eq!(
        find_term("caf\u{e9}.wav", "cafe", &folded, false, false, true),
        // `\u{e9}` is two bytes in UTF-8, so the match spans 5 bytes.
        Some((0, 5))
    );
}
