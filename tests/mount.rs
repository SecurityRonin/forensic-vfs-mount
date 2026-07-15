//! Integration tests for `MountFs` against a real `FileSystem` — `ad1-core`'s
//! `Ad1Vfs` mounted over the spec-faithful `testfix` sample tree (ground truth).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ad1::testfix;
use ad1::Ad1Vfs;
use forensic_vfs::{DynFs, NodeKind};
use forensic_vfs_mount::MountFs;

/// Build the canonical sample tree, write it to a tempdir as `image.ad1`, open it
/// through `Ad1Vfs`, and wrap it in a `MountFs`. Returns the tempdir (kept alive),
/// the mount adapter, and the builder's expected per-entry facts (ground truth).
fn open_sample() -> (tempfile::TempDir, MountFs, Vec<testfix::Expected>) {
    let built = testfix::build(testfix::sample_tree());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("image.ad1");
    std::fs::write(&path, &built.bytes).unwrap();
    let fs = Ad1Vfs::open(&path).unwrap();
    let dynfs: DynFs = Arc::new(fs);
    (dir, MountFs::new(dynfs), built.expected)
}

fn expected_of<'a>(exp: &'a [testfix::Expected], path: &str) -> &'a testfix::Expected {
    exp.iter().find(|e| e.path == path).expect("expected entry")
}

/// Resolve a `/`-separated path from the root inode via `lookup`.
fn resolve(m: &MountFs, parts: &[&[u8]]) -> u64 {
    let mut ino = m.root_ino();
    for p in parts {
        ino = m.lookup(ino, p).unwrap().unwrap();
    }
    ino
}

#[test]
fn read_dir_root_lists_sample_entries_with_stable_inodes() {
    let (_d, m, _e) = open_sample();
    let items = m.read_dir(m.root_ino()).unwrap();
    let root = items
        .iter()
        .find(|it| it.name == b"root")
        .expect("root dir listed");
    assert_eq!(root.kind, NodeKind::Dir);
    // Every assigned inode is fresh (not the root) and unique.
    for it in &items {
        assert_ne!(it.ino, m.root_ino());
    }
    let mut inos: Vec<u64> = items.iter().map(|it| it.ino).collect();
    let n = inos.len();
    inos.sort_unstable();
    inos.dedup();
    assert_eq!(inos.len(), n, "child inodes are unique");
}

#[test]
fn same_file_yields_same_inode_across_lookups() {
    let (_d, m, _e) = open_sample();
    let root = m.root_ino();
    let a = m.lookup(root, b"root").unwrap().unwrap();
    let b = m.lookup(root, b"root").unwrap().unwrap();
    assert_eq!(a, b, "stable inode across two lookups");

    // And a nested file keeps its inode across a lookup and a read_dir listing.
    let hello_via_lookup = resolve(&m, &[b"root", b"hello.txt"]);
    let root_dir = m.lookup(root, b"root").unwrap().unwrap();
    let hello_via_readdir = m
        .read_dir(root_dir)
        .unwrap()
        .into_iter()
        .find(|it| it.name == b"hello.txt")
        .unwrap()
        .ino;
    assert_eq!(hello_via_lookup, hello_via_readdir);
}

#[test]
fn meta_size_matches_ground_truth() {
    let (_d, m, exp) = open_sample();
    let e = expected_of(&exp, "root/hello.txt");
    let ino = resolve(&m, &[b"root", b"hello.txt"]);
    let meta = m.meta(ino).unwrap();
    assert_eq!(meta.kind, NodeKind::File);
    assert_eq!(meta.size, e.size);
}

#[test]
fn read_whole_multichunk_file_is_byte_identical() {
    let (_d, m, exp) = open_sample();
    // `a.bin` is 200 000 bytes (> one 64 KiB chunk) so read() drives the
    // read_at fill loop across multiple underlying chunks.
    let e = expected_of(&exp, "root/sub/a.bin");
    let want = e.data.as_ref().unwrap();
    let ino = resolve(&m, &[b"root", b"sub", b"a.bin"]);
    let got = m.read(ino, 0, e.size).unwrap();
    assert_eq!(got.len(), want.len());
    assert_eq!(&got, want, "whole-file bytes match ground truth");
}

#[test]
fn read_mid_file_window_is_correct() {
    let (_d, m, exp) = open_sample();
    let e = expected_of(&exp, "root/sub/a.bin");
    let want = e.data.as_ref().unwrap();
    let ino = resolve(&m, &[b"root", b"sub", b"a.bin"]);
    let mid = 100_003u64; // deliberately mid-chunk and unaligned
    let n = 40_000u64;
    let got = m.read(ino, mid, n).unwrap();
    let start = usize::try_from(mid).unwrap();
    let end = start + usize::try_from(n).unwrap();
    assert_eq!(&got, &want[start..end], "mid-file window matches");
}

#[test]
fn read_past_eof_is_short_and_capped() {
    let (_d, m, exp) = open_sample();
    let e = expected_of(&exp, "root/hello.txt");
    let ino = resolve(&m, &[b"root", b"hello.txt"]);
    // Ask for far more than the file holds: result is capped to remaining bytes.
    let got = m.read(ino, 0, 1_000_000).unwrap();
    assert_eq!(got.len() as u64, e.size);
    // Reading exactly at EOF yields nothing.
    let empty = m.read(ino, e.size, 4096).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn unknown_inode_errors_without_panic() {
    let (_d, m, _e) = open_sample();
    let bogus = 999_999u64;
    assert!(m.meta(bogus).is_err());
    assert!(m.read(bogus, 0, 16).is_err());
    assert!(m.read_dir(bogus).is_err());
    assert!(m.lookup(bogus, b"x").is_err());
    assert!(m.read_link(bogus).is_err());
}

#[test]
fn read_on_directory_errors_cleanly() {
    let (_d, m, _e) = open_sample();
    let dir = resolve(&m, &[b"root", b"sub"]);
    assert!(m.read(dir, 0, 16).is_err());
}

#[test]
fn read_link_on_non_symlink_errors_cleanly() {
    let (_d, m, _e) = open_sample();
    let file = resolve(&m, &[b"root", b"hello.txt"]);
    assert!(m.read_link(file).is_err());
}
