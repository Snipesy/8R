//! LibDB: library fingerprints forged from scenario builds of an app's exact build profile
//! (docs/research/libdb.md). `8r-forge` writes packs; 8R reads them.

pub mod pack;
pub mod profile;

pub use pack::Pack;
pub use profile::{Coord, Profile};
