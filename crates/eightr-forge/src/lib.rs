//! `8r-forge`: forges LibDB packs (docs/research/libdb.md). From an app's build profile (R8
//! version, min-api, mode, declared library versions) it resolves the exact library closure,
//! builds it with that R8 in generated scenarios, and fingerprints what R8 emitted, keyed by the
//! scenario mappings. 8R itself stays offline: it only reads packs.

pub mod api;
pub mod artifacts;
pub mod build;
pub mod catalog;
pub mod fingerprint;
pub mod gen;
pub mod grade;
pub mod maven;
pub mod names;
pub mod tools;
