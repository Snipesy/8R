use std::collections::BTreeMap;

use eightr_rules::{Attribute, Class, Source, REGISTRY};
use serde::Serialize;

use crate::marker::Marker;
use crate::sources::DetectedSource;

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub eightr_version: &'static str,
    pub inputs: Vec<InputSummary>,
    pub markers: Vec<Marker>,
    pub sources: Vec<DetectedSource>,
    pub findings: Vec<Finding>,
    pub rules: Vec<RuleUsage>,
    /// Per attribute: how many (item, attribute) pairs ended up S, D, N, or untouched.
    pub summary: BTreeMap<Attribute, Counts>,
    /// Structural rewrites performed (outlines inlined back, classes split, ...).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rewrites: Vec<crate::rewrites::RewriteRecord>,
    /// Enums R8 unboxed: original names and constants recovered from what survives.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub enums: Vec<crate::passes::enum_unboxing::RecoveredEnum>,
    /// Evidence of inlining (hints by kind; D·id: nothing is un-inlined).
    pub inlining: crate::inline_hints::InliningSummary,
    /// Every inlining hint, when `Config::verbose_labels` is set.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inline_hints: Vec<crate::inline_hints::InlineHint>,
    /// Libraries in the app and their versions, with the evidence for each.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub libraries: Vec<LibraryVersion>,
    /// kotlinx.serialization descriptors (serial names, element names), when verbose; always
    /// counted in `serialization_descriptors`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub serialization: Vec<SerialDescriptor>,
    pub serialization_descriptors: u64,
    /// Room entity classes named by the schema validation messages (original FQNs).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub room_entities: Vec<String>,
    /// Compose: restartable composables and what their synthetic parameters prove.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose: Option<crate::composables::ComposeSummary>,
    /// Every composable's roles and bindings, when `Config::verbose_labels` is set.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub composables: Vec<crate::composables::Composable>,
    /// Every non-identity label, when `Config::verbose_labels` is set.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<LabelEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputSummary {
    pub name: String,
    pub sha256: String,
    pub dex_version: u32,
    pub classes: u32,
    pub checksum_ok: bool,
    pub signature_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleUsage {
    pub id: &'static str,
    pub source: Source,
    pub class: Class,
    pub applications: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub solved: u64,
    pub deterministic: u64,
    pub nondeterministic: u64,
    /// Not touched by any rule: implicitly `core/identity` (D).
    pub untouched: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabelEntry {
    pub item: String,
    pub attribute: Attribute,
    pub class: Class,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub rules: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<String>>,
}

impl Report {
    pub fn rule_usage(applications: &BTreeMap<&'static str, u64>) -> Vec<RuleUsage> {
        REGISTRY
            .iter()
            .map(|r| RuleUsage {
                id: r.id,
                source: r.source,
                class: r.class,
                applications: applications.get(r.id).copied().unwrap_or(0),
            })
            .collect()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("report serializes") + "\n"
    }
}

/// A library found in the app, with the versions its evidence allows.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct LibraryVersion {
    /// Artifact or library name (`material3`, `okhttp`, `kotlinx_coroutines_core`, ...).
    pub library: String,
    /// Every version the evidence is consistent with (one when exact).
    pub versions: Vec<String>,
    /// What says so, e.g. `compose keys 112/151` or `META-INF/x.version`.
    pub evidence: String,
}

/// A kotlinx.serialization descriptor: the serializer class (input name), the serial name, the
/// element names in order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct SerialDescriptor {
    pub serializer: String,
    pub serial_name: String,
    pub elements: Vec<String>,
}
