//! Synthetic NTFS images (512 B sectors and clusters, 1 KiB records) for the
//! hostile-volume regression tests.
const REC: usize = 1024;
fn p16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes())
}
fn p32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes())
}
fn p64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes())
}

fn record(flags: u16, seq: u16, attrs: &[Vec<u8>]) -> Vec<u8> {
    let mut r = vec![0u8; REC];
    r[0..4].copy_from_slice(b"FILE");
    p16(&mut r, 4, 48);
    p16(&mut r, 6, 3);
    p16(&mut r, 16, seq);
    p16(&mut r, 20, 56);
    p16(&mut r, 22, flags);
    r[48] = 0xaa;
    r[49] = 0xbb;
    let mut p = 56;
    for a in attrs {
        r[p..p + a.len()].copy_from_slice(a);
        p += a.len();
    }
    p32(&mut r, p, 0xffff_ffff);
    p32(&mut r, 24, (p + 8) as u32);
    p32(&mut r, 28, REC as u32);
    for e in [510, 1022] {
        r[e] = 0xaa;
        r[e + 1] = 0xbb;
    }
    r
}
fn resident(typ: u32, value: &[u8]) -> Vec<u8> {
    let len = (24 + value.len() + 7) & !7;
    let mut a = vec![0u8; len];
    p32(&mut a, 0, typ);
    p32(&mut a, 4, len as u32);
    p32(&mut a, 16, value.len() as u32);
    p16(&mut a, 20, 24);
    a[24..24 + value.len()].copy_from_slice(value);
    a
}
fn nonres(typ: u32, id: u16, vcn: u64, sizes: (u64, u64), runs: &[u8]) -> Vec<u8> {
    let len = (64 + runs.len() + 7) & !7;
    let mut a = vec![0u8; len];
    p32(&mut a, 0, typ);
    p32(&mut a, 4, len as u32);
    a[8] = 1;
    p16(&mut a, 14, id);
    p64(&mut a, 16, vcn);
    p16(&mut a, 32, 64);
    p64(&mut a, 40, sizes.0);
    p64(&mut a, 48, sizes.1);
    a[64..64 + runs.len()].copy_from_slice(runs);
    a
}
fn file_name_ref(parent: u64, parent_sequence: u16, name: &str, dir: bool, ns: u8) -> Vec<u8> {
    let w: Vec<u16> = name.encode_utf16().collect();
    let mut v = vec![0u8; 66 + w.len() * 2];
    p64(&mut v, 0, parent | ((parent_sequence as u64) << 48));
    p64(&mut v, 48, 5);
    p32(&mut v, 56, if dir { 0x1000_0000 } else { 0 });
    v[64] = w.len() as u8;
    v[65] = ns;
    for (i, c) in w.iter().enumerate() {
        p16(&mut v, 66 + i * 2, *c);
    }
    resident(0x30, &v)
}
fn file_name(parent: u64, name: &str, dir: bool, ns: u8) -> Vec<u8> {
    file_name_ref(parent, 1, name, dir, ns)
}
fn user_records(recs: &mut [Vec<u8>]) {
    let mut si = vec![0u8; 48];
    p64(&mut si, 8, 133_000_000_000_000_000);
    recs[5] = record(3, 1, &[file_name(5, ".", true, 1)]);
    recs[6] = record(3, 1, &[file_name(5, "docs", true, 1)]);
    recs[7] = record(
        1,
        1,
        &[
            resident(0x10, &si),
            file_name(6, "a.txt", false, 1),
            file_name(6, "b.txt", false, 0),
            resident(0x80, b"hello"),
        ],
    );
}
fn boot() -> Vec<u8> {
    let mut b = vec![0u8; 2048];
    b[3..7].copy_from_slice(b"NTFS");
    p16(&mut b, 11, 512);
    b[13] = 1;
    p64(&mut b, 48, 4);
    b[64] = 0xf6;
    b
}
fn assemble(recs: Vec<Vec<u8>>) -> Vec<u8> {
    let mut img = boot();
    for r in recs {
        img.extend_from_slice(&r);
    }
    img
}

/// Image whose `$MFT` needs an attribute list; `list_size` is the real size
/// recorded for that list (the valid value is 32).
pub fn attr_list_image(list_size: u64) -> Vec<u8> {
    let mut recs = vec![vec![0u8; REC]; 8];
    recs[0] = record(
        1,
        1,
        &[
            nonres(0x80, 0, 0, (4096, 8192), &[0x11, 8, 4, 0]),
            nonres(0x20, 1, 0, (512, list_size), &[0x11, 1, 20, 0]),
        ],
    );
    recs[3] = record(1, 1, &[nonres(0x80, 0, 8, (4096, 4096), &[0x11, 8, 12, 0])]);
    user_records(&mut recs);
    let mut img = assemble(recs);
    let mut list = vec![0u8; 512];
    p32(&mut list, 0, 0x80);
    p16(&mut list, 4, 32);
    p64(&mut list, 8, 8);
    p64(&mut list, 16, 3 | (1u64 << 48));
    img.extend_from_slice(&list);
    img
}

/// A 10 KiB image whose `$MFT` claims 4 GiB through a forged run length.
pub fn declared_huge_mft_image() -> Vec<u8> {
    let mut recs = vec![vec![0u8; REC]; 8];
    let clusters: u64 = 8 << 20;
    let mut runs = vec![0x15u8];
    runs.extend_from_slice(&clusters.to_le_bytes()[..5]);
    runs.extend_from_slice(&[4, 0]);
    let bytes = clusters * 512;
    recs[0] = record(1, 1, &[nonres(0x80, 0, 0, (bytes, bytes), &runs)]);
    user_records(&mut recs);
    assemble(recs)
}

/// Valid volume with one ordinary named record whose parent relationship is
/// deliberately malformed. Parent ids are MFT record numbers; fixture sizes
/// are kept minimal so these reach namespace resolution.
pub fn unresolved_parent_image(kind: &str) -> Vec<u8> {
    let mut recs = vec![vec![0u8; REC]; 9];
    recs[0] = record(1, 1, &[nonres(0x80, 0, 0, (9216, 9216), &[0x11, 18, 4, 0])]);
    recs[5] = record(3, 1, &[file_name(5, ".", true, 1)]);
    match kind {
        "missing" => {
            recs[8] = record(1, 1, &[file_name(99, "lost", false, 1)]);
        }
        "sequence" => {
            recs[6] = record(3, 2, &[file_name(5, "docs", true, 1)]);
            recs[8] = record(1, 1, &[file_name_ref(6, 1, "lost", false, 1)]);
        }
        "cycle" => {
            recs[6] = record(3, 1, &[file_name(7, "a", true, 1)]);
            recs[7] = record(3, 1, &[file_name(6, "b", true, 1)]);
            recs[8] = record(1, 1, &[file_name(6, "lost", false, 1)]);
        }
        "root-sequence" => {
            recs[6] = record(3, 1, &[file_name_ref(5, 99, "docs", true, 1)]);
        }
        "alias-missing" => {
            recs[6] = record(3, 1, &[file_name(5, "docs", true, 1)]);
            recs[8] = record(
                1,
                1,
                &[
                    file_name(6, "ok.txt", false, 1),
                    file_name_ref(99, 1, "ghost.txt", false, 1),
                ],
            );
        }
        "alias-sequence" => {
            recs[6] = record(3, 2, &[file_name(5, "docs", true, 1)]);
            recs[8] = record(
                1,
                1,
                &[
                    file_name_ref(6, 2, "ok.txt", false, 1),
                    file_name_ref(6, 1, "ghost.txt", false, 1),
                ],
            );
        }
        _ => unreachable!("unknown fixture kind"),
    }
    assemble(recs)
}
