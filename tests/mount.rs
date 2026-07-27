//! Integration tests for the generic `MountFs` adapter.
//!
//! `MountFs` is format-agnostic: it only ever calls the `forensic_vfs::FileSystem`
//! trait (`root`/`read_dir`/`lookup`/`meta`/`read_at`/`read_link`) and never
//! touches any container's bytes. So the unit under test is fully exercised by a
//! small in-repo `MockFs` whose tree + file contents are the ground truth — this
//! keeps CI self-contained (no sibling-repo path dependency) while covering the
//! same behavior a real filesystem would: stable/unique inode interning, the
//! short-read-safe whole-file fill loop across multiple `read_at` chunks, EOF
//! capping, and loud errors on misuse. (Validating a concrete reader such as
//! `Ad1Vfs` against real-artifact ground truth belongs in that reader's own repo,
//! not in this generic adapter's suite.)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;

use forensic_vfs::{
    Allocation, DirEntry, DirStream, DynFs, ExtentStream, FileId, FileSystem, FsKind, FsMeta,
    MacbTimes, NodeKind, NodeStream, ResidencyKind, SectorSizes, StreamId, TimeZonePolicy,
    VfsError, VfsResult,
};
use forensic_vfs_mount::MountFs;

// --- Ground truth ---------------------------------------------------------

const HELLO: &[u8] = b"hello, world\n";
/// `a.bin` is 200 000 bytes (> one 64 KiB chunk) so a whole-file read drives the
/// adapter's `read_at` fill loop across multiple underlying chunks.
const ABIN_LEN: u64 = 200_000;

fn abin_data() -> Vec<u8> {
    (0..ABIN_LEN)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect()
}

// --- A minimal in-memory `FileSystem` -------------------------------------

/// The mock's underlying `read_at` returns at most this many bytes per call, so a
/// whole-file read of `a.bin` must loop several times — exactly the short-read
/// behavior the adapter's fill loop exists to absorb.
const CHUNK: usize = 64 * 1024;

struct MockNode {
    kind: NodeKind,
    /// File bytes; `None` for a directory. For a symlink, the target path bytes.
    data: Option<Vec<u8>>,
    /// `(name, child inode)` pairs; empty for a file.
    children: Vec<(Vec<u8>, u64)>,
    /// When set, `meta().size` reports this instead of `data.len()`. Models a
    /// filesystem whose metadata overstates the readable bytes (a sparse or
    /// concurrently-truncated file), so the whole-file fill loop must terminate
    /// on the underlying `read_at` returning 0, not spin.
    size_override: Option<u64>,
}

/// A tiny read-only tree addressed by [`FileId::Opaque`]:
///
/// ```text
/// /            (inode 1, mount root)
///   root/      (inode 2)
///     hello.txt (inode 3)
///     sub/      (inode 4)
///       a.bin   (inode 5)
/// ```
struct MockFs {
    nodes: HashMap<u64, MockNode>,
}

impl MockFs {
    const ROOT: u64 = 1;

    fn sample() -> Self {
        let mut nodes = HashMap::new();
        nodes.insert(
            1,
            MockNode {
                kind: NodeKind::Dir,
                data: None,
                children: vec![(b"root".to_vec(), 2)],
                size_override: None,
            },
        );
        nodes.insert(
            2,
            MockNode {
                kind: NodeKind::Dir,
                data: None,
                children: vec![
                    (b"hello.txt".to_vec(), 3),
                    (b"sub".to_vec(), 4),
                    (b"link".to_vec(), 6),
                ],
                size_override: None,
            },
        );
        nodes.insert(
            3,
            MockNode {
                kind: NodeKind::File,
                data: Some(HELLO.to_vec()),
                children: Vec::new(),
                size_override: None,
            },
        );
        nodes.insert(
            4,
            MockNode {
                kind: NodeKind::Dir,
                data: None,
                children: vec![(b"a.bin".to_vec(), 5), (b"sparse.bin".to_vec(), 7)],
                size_override: None,
            },
        );
        nodes.insert(
            5,
            MockNode {
                kind: NodeKind::File,
                data: Some(abin_data()),
                children: Vec::new(),
                size_override: None,
            },
        );
        // A symlink whose target is the path bytes stored in `data`.
        nodes.insert(
            6,
            MockNode {
                kind: NodeKind::Symlink,
                data: Some(b"/root/hello.txt".to_vec()),
                children: Vec::new(),
                size_override: None,
            },
        );
        // A file whose metadata claims 100 bytes but only 10 are readable: the
        // fill loop must stop when read_at returns 0, not spin to the declared
        // size.
        nodes.insert(
            7,
            MockNode {
                kind: NodeKind::File,
                data: Some(vec![0xABu8; 10]),
                children: Vec::new(),
                size_override: Some(100),
            },
        );
        Self { nodes }
    }

    fn node(&self, id: FileId) -> VfsResult<&MockNode> {
        let key = match id {
            FileId::Opaque(k) => k,
            other => {
                return Err(VfsError::Unsupported {
                    layer: "mock",
                    scheme: format!("unexpected FileId {other:?}"),
                })
            }
        };
        self.nodes.get(&key).ok_or(VfsError::Unsupported {
            layer: "mock",
            scheme: format!("unknown file id {key}"),
        })
    }
}

fn meta_of(ino: u64, n: &MockNode) -> FsMeta {
    let size = n
        .size_override
        .unwrap_or_else(|| n.data.as_ref().map_or(0, |d| d.len() as u64));
    FsMeta {
        ino,
        kind: n.kind,
        allocated: Allocation::Allocated,
        size,
        nlink: 1,
        uid: None,
        gid: None,
        mode: None,
        times: MacbTimes::default(),
        streams: Vec::new(),
        residency: ResidencyKind::NonResident,
        link_target: None,
    }
}

impl FileSystem for MockFs {
    fn kind(&self) -> FsKind {
        FsKind::from_name("mock")
    }

    fn root(&self) -> FileId {
        FileId::Opaque(Self::ROOT)
    }

    fn sector_sizes(&self) -> SectorSizes {
        SectorSizes {
            logical: 512,
            physical: 512,
            cluster_or_block: 512,
        }
    }

    fn timestamp_zone(&self) -> TimeZonePolicy {
        TimeZonePolicy::Utc
    }

    fn read_dir(&self, ino: FileId) -> VfsResult<DirStream> {
        let n = self.node(ino)?;
        let entries: Vec<VfsResult<DirEntry>> = n
            .children
            .iter()
            .map(|(name, child)| {
                let kind = self.nodes.get(child).map_or(NodeKind::File, |c| c.kind);
                Ok(DirEntry {
                    name: name.clone(),
                    id: FileId::Opaque(*child),
                    kind,
                })
            })
            .collect();
        Ok(DirStream::new(entries.into_iter()))
    }

    fn extents(&self, _ino: FileId, _stream: StreamId) -> VfsResult<ExtentStream> {
        Ok(ExtentStream::empty())
    }

    fn lookup(&self, parent: FileId, name: &[u8]) -> VfsResult<Option<FileId>> {
        let n = self.node(parent)?;
        Ok(n.children
            .iter()
            .find(|(cname, _)| cname.as_slice() == name)
            .map(|(_, child)| FileId::Opaque(*child)))
    }

    fn meta(&self, ino: FileId) -> VfsResult<FsMeta> {
        let key = match ino {
            FileId::Opaque(k) => k,
            other => {
                return Err(VfsError::Unsupported {
                    layer: "mock",
                    scheme: format!("unexpected FileId {other:?}"),
                })
            }
        };
        let n = self.node(ino)?;
        Ok(meta_of(key, n))
    }

    fn read_at(
        &self,
        ino: FileId,
        _stream: StreamId,
        off: u64,
        buf: &mut [u8],
    ) -> VfsResult<usize> {
        let n = self.node(ino)?;
        let Some(data) = n.data.as_ref() else {
            return Err(VfsError::Unsupported {
                layer: "mock",
                scheme: "read_at on a directory".to_string(),
            });
        };
        let start = usize::try_from(off).unwrap_or(usize::MAX);
        let Some(rest) = data.get(start..) else {
            return Ok(0); // past EOF
        };
        let n = rest.len().min(buf.len()).min(CHUNK);
        buf[..n].copy_from_slice(&rest[..n]);
        Ok(n)
    }

    fn read_link(&self, ino: FileId, _cap: usize) -> VfsResult<Vec<u8>> {
        let n = self.node(ino)?;
        if n.kind == NodeKind::Symlink {
            if let Some(target) = n.data.as_ref() {
                return Ok(target.clone());
            }
        }
        Err(VfsError::Unsupported {
            layer: "mock",
            scheme: "not a symlink".to_string(),
        })
    }

    fn deleted(&self) -> VfsResult<NodeStream> {
        Ok(NodeStream::empty())
    }

    fn unallocated(&self) -> VfsResult<ExtentStream> {
        Ok(ExtentStream::empty())
    }
}

// --- Test harness ---------------------------------------------------------

fn open_sample() -> MountFs {
    let dynfs: DynFs = Arc::new(MockFs::sample());
    MountFs::new(dynfs)
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
    let m = open_sample();
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
    let m = open_sample();
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
    let m = open_sample();
    let ino = resolve(&m, &[b"root", b"hello.txt"]);
    let meta = m.meta(ino).unwrap();
    assert_eq!(meta.kind, NodeKind::File);
    assert_eq!(meta.size, HELLO.len() as u64);
}

#[test]
fn read_whole_multichunk_file_is_byte_identical() {
    let m = open_sample();
    let want = abin_data();
    let ino = resolve(&m, &[b"root", b"sub", b"a.bin"]);
    let got = m.read(ino, 0, ABIN_LEN).unwrap();
    assert_eq!(got.len(), want.len());
    assert_eq!(got, want, "whole-file bytes match ground truth");
}

#[test]
fn read_mid_file_window_is_correct() {
    let m = open_sample();
    let want = abin_data();
    let ino = resolve(&m, &[b"root", b"sub", b"a.bin"]);
    let mid = 100_003u64; // deliberately mid-chunk and unaligned
    let n = 40_000u64;
    let got = m.read(ino, mid, n).unwrap();
    let start = usize::try_from(mid).unwrap();
    let end = start + usize::try_from(n).unwrap();
    assert_eq!(got, want[start..end], "mid-file window matches");
}

#[test]
fn read_past_eof_is_short_and_capped() {
    let m = open_sample();
    let ino = resolve(&m, &[b"root", b"hello.txt"]);
    // Ask for far more than the file holds: result is capped to remaining bytes.
    let got = m.read(ino, 0, 1_000_000).unwrap();
    assert_eq!(got.len() as u64, HELLO.len() as u64);
    // Reading exactly at EOF yields nothing.
    let empty = m.read(ino, HELLO.len() as u64, 4096).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn unknown_inode_errors_without_panic() {
    let m = open_sample();
    let bogus = 999_999u64;
    assert!(m.meta(bogus).is_err());
    assert!(m.read(bogus, 0, 16).is_err());
    assert!(m.read_dir(bogus).is_err());
    assert!(m.lookup(bogus, b"x").is_err());
    assert!(m.read_link(bogus).is_err());
}

#[test]
fn read_on_directory_errors_cleanly() {
    let m = open_sample();
    let dir = resolve(&m, &[b"root", b"sub"]);
    assert!(m.read(dir, 0, 16).is_err());
}

#[test]
fn read_link_on_non_symlink_errors_cleanly() {
    let m = open_sample();
    let file = resolve(&m, &[b"root", b"hello.txt"]);
    assert!(m.read_link(file).is_err());
}

#[test]
fn read_link_on_symlink_returns_target() {
    let m = open_sample();
    let link = resolve(&m, &[b"root", b"link"]);
    let target = m.read_link(link).unwrap();
    assert_eq!(target, b"/root/hello.txt");
}

#[test]
fn lookup_missing_name_returns_none() {
    let m = open_sample();
    let root_dir = m.lookup(m.root_ino(), b"root").unwrap().unwrap();
    // A name that isn't a child resolves to Ok(None) — not an error, not a panic.
    assert_eq!(m.lookup(root_dir, b"nonexistent").unwrap(), None);
}

#[test]
fn read_terminates_when_declared_size_exceeds_readable_bytes() {
    // `sparse.bin` reports size 100 but only 10 bytes are readable; the fill
    // loop must stop on the underlying read_at returning 0, yielding exactly
    // the 10 available bytes rather than spinning or padding.
    let m = open_sample();
    let ino = resolve(&m, &[b"root", b"sub", b"sparse.bin"]);
    assert_eq!(m.meta(ino).unwrap().size, 100);
    let got = m.read(ino, 0, 100).unwrap();
    assert_eq!(got, vec![0xABu8; 10]);
}
