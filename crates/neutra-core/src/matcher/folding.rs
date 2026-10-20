/// Normalize one haystack character the same way the needle was folded:
/// strip accents when asked, then lowercase unless case-sensitive.
#[inline]
pub(super) fn norm_char(c: char, case_sensitive: bool, fold_accents: bool) -> char {
    let mut c = if fold_accents { strip_accent(c) } else { c };
    if !case_sensitive {
        c = c.to_lowercase().next().unwrap_or(c);
    }
    c
}

/// Strip common Latin diacritics to their base letter, preserving case so
/// Strip common Latin diacritics to their base letter, preserving case so
/// case-sensitive search still tells `E-acute` from `e`. Unlisted scripts
/// pass through untouched (same semantics as before for those).
// ponytail: Latin-only accent table, upgrade to NFKD folding if non-Latin
// accent-insensitive search is ever requested.
#[inline]
pub(super) fn strip_accent(c: char) -> char {
    match c {
        '\u{e0}' | '\u{e1}' | '\u{e2}' | '\u{e3}' | '\u{e4}' | '\u{e5}' | '\u{101}' | '\u{103}'
        | '\u{105}' => 'a',
        '\u{c0}' | '\u{c1}' | '\u{c2}' | '\u{c3}' | '\u{c4}' | '\u{c5}' | '\u{100}' | '\u{102}'
        | '\u{104}' => 'A',
        '\u{e8}' | '\u{e9}' | '\u{ea}' | '\u{eb}' | '\u{113}' | '\u{115}' | '\u{117}'
        | '\u{119}' | '\u{11b}' => 'e',
        '\u{c8}' | '\u{c9}' | '\u{ca}' | '\u{cb}' | '\u{112}' | '\u{114}' | '\u{116}'
        | '\u{118}' | '\u{11a}' => 'E',
        '\u{ec}' | '\u{ed}' | '\u{ee}' | '\u{ef}' | '\u{12b}' | '\u{12d}' | '\u{12f}'
        | '\u{131}' => 'i',
        '\u{cc}' | '\u{cd}' | '\u{ce}' | '\u{cf}' | '\u{12a}' | '\u{12c}' | '\u{12e}'
        | '\u{130}' => 'I',
        '\u{f2}' | '\u{f3}' | '\u{f4}' | '\u{f5}' | '\u{f6}' | '\u{f8}' | '\u{14d}' | '\u{14f}'
        | '\u{151}' => 'o',
        '\u{d2}' | '\u{d3}' | '\u{d4}' | '\u{d5}' | '\u{d6}' | '\u{d8}' | '\u{14c}' | '\u{14e}'
        | '\u{150}' => 'O',
        '\u{f9}' | '\u{fa}' | '\u{fb}' | '\u{fc}' | '\u{16b}' | '\u{16d}' | '\u{16f}'
        | '\u{171}' | '\u{173}' => 'u',
        '\u{d9}' | '\u{da}' | '\u{db}' | '\u{dc}' | '\u{16a}' | '\u{16c}' | '\u{16e}'
        | '\u{170}' | '\u{172}' => 'U',
        '\u{fd}' | '\u{ff}' | '\u{177}' => 'y',
        '\u{dd}' | '\u{178}' | '\u{176}' => 'Y',
        '\u{e7}' | '\u{107}' | '\u{109}' | '\u{10d}' => 'c',
        '\u{c7}' | '\u{106}' | '\u{108}' | '\u{10c}' => 'C',
        '\u{f1}' | '\u{144}' | '\u{146}' | '\u{148}' => 'n',
        '\u{d1}' | '\u{143}' | '\u{145}' | '\u{147}' => 'N',
        '\u{15b}' | '\u{15d}' | '\u{15f}' | '\u{161}' => 's',
        '\u{15a}' | '\u{15c}' | '\u{15e}' | '\u{160}' => 'S',
        '\u{17a}' | '\u{17c}' | '\u{17e}' => 'z',
        '\u{179}' | '\u{17b}' | '\u{17d}' => 'Z',
        '\u{11d}' | '\u{11f}' | '\u{121}' | '\u{123}' => 'g',
        '\u{11c}' | '\u{11e}' | '\u{120}' | '\u{122}' => 'G',
        '\u{13a}' | '\u{13c}' | '\u{13e}' | '\u{140}' | '\u{142}' => 'l',
        '\u{139}' | '\u{13b}' | '\u{13d}' | '\u{13f}' | '\u{141}' => 'L',
        '\u{155}' | '\u{157}' | '\u{159}' => 'r',
        '\u{154}' | '\u{156}' | '\u{158}' => 'R',
        _ => c,
    }
}

/// Strip accents from a whole string (needle pre-folding).
pub(super) fn strip_accents_str(s: &str) -> String {
    s.chars().map(strip_accent).collect()
}
