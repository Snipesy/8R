//! Library signature DB (DESIGN §5.4, docs/research/sigdb.md): name-independent fingerprints of
//! library methods as R8 leaves them, and matching an app's code against them.

pub mod db;
pub mod print;
pub mod matcher;
