//! Detecting which sources (tools/plugins) shaped the input, from evidence in the dex.
//!
//! Detection reports positive evidence only. Absence of evidence is not evidence of absence:
//! R8 renames library classes too, so e.g. an obfuscated kotlinx.serialization runtime won't
//! show up by name. Structural fingerprints will extend this.

use std::collections::{BTreeMap, BTreeSet};

use eightr_dex::Dex;
use eightr_rules::Source;
use serde::Serialize;

use crate::marker::Marker;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetectedSource {
    pub source: Source,
    /// Sorted, deduplicated, capped: short descriptions of what was seen.
    pub evidence: Vec<String>,
}

const MAX_EVIDENCE: usize = 8;

/// (source, descriptor prefix) pairs: referencing any type with the prefix is evidence.
const TYPE_PREFIXES: &[(Source, &str)] = &[
    (Source::Kotlinc, "Lkotlin/Metadata;"),
    (Source::Kotlinc, "Lkotlin/jvm/internal/Intrinsics;"),
    (Source::KotlinxSerialization, "Lkotlinx/serialization/"),
    (Source::Compose, "Landroidx/compose/runtime/Composer;"),
    (Source::Compose, "Landroidx/compose/runtime/internal/ComposableLambda"),
    (Source::Parcelize, "Lkotlinx/parcelize/"),
];

/// Substrings of class descriptors characteristic of D8/R8 desugaring synthetics.
const DESUGAR_MARKERS: &[&str] = &[
    "$$ExternalSyntheticLambda",
    "$$ExternalSyntheticBackport",
    "$$ExternalSyntheticApiModelOutline",
    "$$ExternalSyntheticOutline",
    "$$ExternalSyntheticThrowCCEIfNotNull",
    "$-CC;",
    "-$$Nest$",
    "$r8$lambda$",
];

pub fn detect(dexes: &[(String, Dex)], markers: &[Marker]) -> Vec<DetectedSource> {
    let mut found: BTreeMap<Source, BTreeSet<String>> = BTreeMap::new();
    for m in markers {
        let source = match m.tool.as_str() {
            "R8" => Source::R8,
            _ => Source::Desugar,
        };
        let what = format!("~~{} marker, version {}", m.tool, m.version().unwrap_or("?"));
        found.entry(source).or_default().insert(what);
    }
    for (_, dex) in dexes {
        for i in 0..dex.type_count() {
            let Ok(t) = dex.type_descriptor(i) else { continue };
            for &(source, prefix) in TYPE_PREFIXES {
                if t.starts_with(prefix) {
                    found.entry(source).or_default().insert(format!("references {prefix}"));
                }
            }
            for marker in DESUGAR_MARKERS {
                if t.contains(marker) {
                    found.entry(Source::Desugar).or_default().insert(format!("synthetic type name containing {marker}"));
                }
            }
        }
    }
    found
        .into_iter()
        .map(|(source, ev)| DetectedSource { source, evidence: ev.into_iter().take(MAX_EVIDENCE).collect() })
        .collect()
}
