# 1. An adapter crate between the VFS contract and a mount handler

Date: 2026-07-24
Status: Accepted

## Context

The fleet's unified VFS contract — `forensic_vfs::FileSystem` (imported here as
`DynFs = Arc<dyn FileSystem>`) — addresses files by an opaque `FileId` enum and
offers only a positioned, possibly-short `read_at(FileId, StreamId, off, buf) ->
usize`. It is the right shape for the stacked-image world it serves: a whole
`E01 → GPT → BitLocker → NTFS` stack reads as one `Arc<dyn ImageSource>` that
many workers share and no path can write (see the `ronin-issen` constitution,
"VFS & Universal Container Abstraction").

A FUSE (Linux/macOS) or Dokan (Windows) mount handler wants a different surface:
**`u64` inodes** it can hand the kernel, and a **whole-file `read`** rather than a
loop that may return fewer bytes than asked. Without a shared adapter, every
mount front-end re-derives the same `FileId ↔ u64` mapping and the same
`read_at` fill loop — duplicated, and each copy an opportunity to get the inode
stability or the short-read handling subtly wrong.

`4n6mount` is the first consumer; its ADR-0006 ("adopt forensic-vfs-mount")
records the intent from the consumer side.

## Decision

Ship one small library crate, `forensic-vfs-mount`, whose sole export is
`MountFs` — a `u64`-inode, whole-file view over any `DynFs`
(`src/lib.rs`). It owns exactly two jobs (detailed in ADR-0002 and ADR-0003):
the stable `u64 ↔ FileId` mapping and the whole-file short-read-safe `read`. It
exposes the handful of operations a mount handler needs — `read_dir`, `lookup`,
`meta`, `read`, `read_link`, `root_ino` — and nothing else.

The crate is **medium-agnostic**: it knows only the VFS contract, never a
container or filesystem format. Whatever `forensic-vfs-engine` composed beneath
the `DynFs` — raw disk, volume system, crypto layer, filesystem — the mount
handler sees one inode-addressed tree.

The name follows the contract it adapts (`forensic-vfs` → `forensic-vfs-mount`),
a companion adapter rather than the single-format `-core`/`-forensic` split or a
multi-crate `-*` suite; neither naming pattern in the constitution's grammar
fits a one-type adapter, so it is named for its role beside its contract crate.

## Consequences

- Every mount front-end depends on this one adapter and stops re-deriving the
  inode map and fill loop; a fix here fixes all consumers.
- The adapter carries no image/filesystem knowledge, so a new container or
  filesystem format added under `forensic-vfs` benefits mounting for free — the
  consumer-special-cases-one-format smell the VFS policy exists to catch cannot
  arise here.
- The surface is deliberately narrow (read-only navigation + read). Write, mkdir,
  rename and the rest of the FUSE/Dokan operation set are out of scope by design
  (forensic mounts are read-only), so they are not modelled.
