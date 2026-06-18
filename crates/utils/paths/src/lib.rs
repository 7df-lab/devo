//! Path helpers shared across configuration, runtime, and tooling crates.
//!
//! The crate centralizes Devo home/config path discovery plus absolute-path
//! normalization so callers do not each carry their own home-directory expansion
//! or relative-path base rules.

pub mod absolute_path;
mod config_paths;
pub mod home_dir;

pub use config_paths::*;
pub use home_dir::*;
