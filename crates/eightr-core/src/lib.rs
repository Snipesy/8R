//! The 8R pipeline. Input is a shipped artifact only (dex/APK/AAB). There is never a
//! mapping file; see DESIGN.md.

pub mod error;
pub mod input;
pub mod labels;
pub mod libdb;
pub mod libraries;
pub mod marker;
pub mod naming;
pub mod output;
pub mod passes;
pub mod pipeline;
pub mod program;
pub mod protobuf;
pub mod report;
pub mod compose;
pub mod composables;
pub mod compose_keys;
pub mod inline_hints;
pub mod rewrites;
pub mod sigdb;
pub mod sources;

pub use error::{Error, Result};
pub use pipeline::{run, Config, Outcome};
