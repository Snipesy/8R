//! Deterministic mutation testing: corrupt real fixture dex files in many ways and require
//! that parsing + full validation returns `Ok` or `Err`, but never panics or hangs.
//! (Coverage-guided fuzzing with cargo-fuzz comes later; this runs on every `cargo test`.)

use std::fs;
use std::path::Path;

use eightr_dex::Dex;

/// Small deterministic PRNG (xorshift64*), so failures reproduce exactly.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn exercise(bytes: &[u8]) {
    if let Ok(dex) = Dex::parse(bytes) {
        let _ = dex.validate();
        let _ = dex.checksum_ok();
    }
}

#[test]
fn mutations_never_panic() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out");
    let mut paths: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .flat_map(|f| ["d8", "r8"].map(|v| f.as_ref().unwrap().path().join(v).join("classes.dex")))
        .filter(|p| p.exists())
        .collect();
    paths.sort();
    let iterations = std::env::var("EIGHTR_MUTATIONS").ok().and_then(|s| s.parse().ok()).unwrap_or(400);

    for path in paths {
        let original = fs::read(&path).unwrap();
        let mut rng = Rng(0x8e8e_8e8e ^ original.len() as u64);
        for i in 0..iterations {
            let mut m = original.clone();
            match i % 4 {
                // Flip a few random bytes anywhere past the magic.
                0 => {
                    for _ in 0..1 + rng.below(8) {
                        let at = 8 + rng.below(m.len() - 8);
                        m[at] ^= 1 << rng.below(8);
                    }
                }
                // Overwrite a random u32 with an extreme value (targets offsets and sizes).
                1 => {
                    let at = rng.below(m.len() - 4) & !3;
                    let v = [0u32, 1, 0x7fff_ffff, 0xffff_ffff, m.len() as u32, m.len() as u32 - 1][rng.below(6)];
                    m[at..at + 4].copy_from_slice(&v.to_le_bytes());
                }
                // Truncate.
                2 => m.truncate(rng.below(m.len())),
                // Corrupt a header field specifically.
                _ => {
                    let at = 0x20 + (rng.below(0x50) & !3);
                    let v = rng.next() as u32;
                    m[at..at + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            let result = std::panic::catch_unwind(|| exercise(&m));
            assert!(result.is_ok(), "{} mutation #{i} panicked", path.display());
        }
    }
}
