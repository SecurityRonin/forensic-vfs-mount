# 2. Stable `u64 ↔ FileId` inode interning

Date: 2026-07-24
Status: Accepted

## Context

FUSE and Dokan require that a given file keep the **same inode for the life of
the mount** — the kernel caches inodes and will re-issue operations against a
number it was handed earlier. The VFS contract identifies a file by a `FileId`
enum, which is not a `u64` and carries no promise of a small dense range. The
adapter must therefore mint stable `u64` inodes and be able to translate a
kernel-supplied inode back to the `FileId` the contract understands, in both
directions, for the whole mount.

The adapter is shared across mount worker threads through `&self`, so the mapping
state has to be interior-mutable without forcing every method to take `&mut self`.

## Decision

`InodeMap` (`src/lib.rs`) is a bidirectional table — `forward: HashMap<FileId,
u64>`, `reverse: HashMap<u64, FileId>`, and a `next` counter — held behind a
`Mutex` on `MountFs` so the whole adapter stays `&self`.

- The root `FileId` (from `fs.root()`) is pre-interned to `MountFs::ROOT_INO =
  1`, the FUSE convention for the mount root.
- Every other `FileId` is interned lazily to a fresh incrementing `u64` the first
  time it is seen (in `read_dir` or `lookup`) and reused thereafter, so repeat
  lookups of the same file return the same inode.
- The counter uses `saturating_add`, so a pathological `2^64`-entry tree stops
  handing out inodes rather than wrapping and aliasing an existing file.
- Resolving an unknown inode returns a loud `VfsError::Unsupported` naming the
  offending inode (`"unknown inode {ino}"`), never a panic and never a silent
  empty result.
- The mutex guard recovers from poisoning
  (`unwrap_or_else(PoisonError::into_inner)`) so a panic in one worker cannot
  wedge the mount by poisoning the lock.

## Consequences

- Inode stability holds by construction: the interning tests
  (`same_file_yields_same_inode_across_lookups` in `tests/mount.rs`) confirm a
  file keeps its inode across repeated `lookup`s and across a `read_dir` listing.
- The `Mutex` serialises inode assignment. For a read-only browse mount this is
  not a hot path; if it ever became one, the table could move to a sharded or
  lock-free map without changing the public surface.
- Inodes are assigned in discovery order, not derived from any on-disk
  identifier, so they are stable only *within* a mount session and are not
  comparable across mounts — which is exactly what FUSE requires and nothing more.
- A `saturating` counter means the `2^64`-th distinct file would collide on
  `u64::MAX`; this is documented as unreachable in practice and preferred over
  silent wraparound aliasing.
