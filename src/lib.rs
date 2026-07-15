//! Generic `forensic_vfs::FileSystem` -> u64-inode / whole-file mount adapter.
//!
//! RED scaffold: the tests are written first and reference [`MountFs`], which is
//! not implemented yet, so `cargo test` fails to compile until the GREEN commit.

#![forbid(unsafe_code)]
