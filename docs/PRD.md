# forensic-vfs-mount — Purpose & Scope

> Library tier (per the `ronin-issen` PRD & ADR Standard): `forensic-vfs-mount`
> ships no binary an examiner runs — it is *linked* by mount front-ends. This is
> the lighter Purpose & Scope doc, not a product PRD.

## What it is

`forensic-vfs-mount` is the keystone adapter between the fleet's unified VFS
contract and a FUSE/Dokan mount handler. Its single export, `MountFs`, wraps any
`Arc<dyn forensic_vfs::FileSystem>` and presents the surface a mount handler
actually wants:

- **Stable `u64` inodes** (the root is inode `1`, the FUSE convention; every other
  file is interned to a fresh, stable inode on first sight) — see ADR-0002.
- **A whole-file, short-read-safe `read`** that loops the contract's positioned
  `read_at` until filled and caps its allocation at the file's own size — see
  ADR-0003.

Everything else — `read_dir`, `lookup`, `meta`, `read_link`, `root_ino` — is the
minimal navigation surface a read-only mount needs.

## Who links it

Mount front-ends. `4n6mount` is the first consumer (its ADR-0006 adopts this
crate). Any tool that wants to present a `forensic-vfs` filesystem — a whole
stacked image (container → volume system → crypto layer → filesystem) or a single
logical container such as AD1 — as an inode-addressed FUSE/Dokan tree links
`forensic-vfs-mount` instead of re-deriving the same inode map and fill loop.

## Where it sits

The layering, top (contract) to bottom (front-end):

1. `forensic-vfs` — the contract: `FileId` + short-read `read_at`, medium-agnostic.
2. `forensic-vfs-mount` — **this crate**: `u64` inodes + whole-file read.
3. mount front-end (e.g. `4n6mount`) — binds the surface to FUSE / Dokan.

The adapter knows only the VFS contract; it never learns one container or
filesystem format from another. Whatever `forensic-vfs-engine` composed beneath
the `DynFs` is what the mount handler browses (ADR-0001).

## Scope

- Translate `FileId ↔ u64` inodes, stably, for the life of a mount.
- Turn the contract's short-read `read_at` into a whole-file, bounded-allocation
  `read`.
- Expose read-only directory navigation, metadata, and symlink-target reads.
- Fail loud on every error path — unknown inode, read-on-directory (`EISDIR`),
  read_link-on-non-symlink (`EINVAL`) — never a panic, never a silent empty result.

## Non-goals

- **No writes.** Forensic mounts are read-only; `mkdir`/`rename`/`unlink`/`write`
  and the rest of the FUSE/Dokan mutation set are deliberately unmodelled.
- **No format knowledge.** The adapter decodes no container or filesystem; that is
  the job of the readers beneath `forensic-vfs` (VFS abstraction policy).
- **No FUSE/Dokan binding.** It produces the *surface* a mount handler consumes;
  wiring that surface to `libfuse`/Dokan and running a mount loop is the front-end's
  job.
- **No caching or overlay/CoW layer.** Copy-on-write browse overlays are the mount
  front-end's concern (e.g. 4n6mount's ADR-0005), not this adapter's.

## Validation approach

Correctness is proven at the integration level against an **independent oracle**,
not a hand-written mock: `tests/mount.rs` mounts `ad1-core`'s `Ad1Vfs` over the
spec-faithful `testfix` sample tree (ground truth) through a real `MountFs` and
checks inode stability across lookups and listings, `meta().size` against the
builder's expected facts, byte-identical whole-file and mid-file-window reads of a
multi-chunk (200 000-byte) file, EOF capping, and that every error path fails
without a panic. `Ad1Vfs` is a genuine `FileSystem` implementation from another
repo, so the tests exercise the adapter against a real contract impl rather than a
fixture written to its own assumptions.
