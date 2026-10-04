//! α-invariance (DESIGN.md §0.3): R8's choice of minified names and of class/file order is
//! arbitrary, so 8R's output must not depend on it (machinery in `support/alpha.rs`).

#[path = "support/alpha.rs"]
mod support;

use eightr_core::Config;

#[test]
fn outputs_are_alpha_invariant() {
    let mut fixtures: Vec<String> = std::fs::read_dir(support::fixtures_root())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    fixtures.sort();
    let seeds = std::env::var("EIGHTR_ALPHA_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6u64);
    let from = std::env::var("EIGHTR_ALPHA_SEED_FROM")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1u64);
    let mut total_renamed = 0;
    for fixture in fixtures {
        if std::env::var("EIGHTR_ALPHA_FIXTURE").is_ok_and(|f| f != fixture) {
            continue;
        }
        total_renamed += support::check(&fixture, &Config::default(), from..=seeds);
    }
    assert!(
        total_renamed > 100,
        "scrambling should actually rename things (renamed {total_renamed})"
    );
}
