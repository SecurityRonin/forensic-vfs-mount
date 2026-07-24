# 3. Whole-file, short-read-safe `read` with bounded allocation

Date: 2026-07-24
Status: Accepted

## Context

The VFS contract offers only `read_at(FileId, StreamId, off, buf) -> usize`,
which is permitted to **short-read**: a single call may fill fewer bytes than the
buffer holds (a chunk boundary, a partial backing read). A FUSE/Dokan `read`
handler, by contrast, is expected to return the requested window in full (up to
EOF). Bridging the two naively invites two bugs: a caller that trusts the first
`read_at` return truncates multi-chunk files, and a caller that allocates a
buffer sized to the kernel's `size` argument can be driven to allocate without
bound by an absurd request.

## Decision

`MountFs::read(ino, offset, size)` (`src/lib.rs`) does three things:

1. **Caps the allocation at the file's own size.** It reads `meta().size`, computes
   `remaining = meta.size.saturating_sub(offset)`, takes `want = size.min(remaining)`,
   and allocates only `want` bytes. An absurd
   `size`, or a read starting past EOF, can never allocate beyond what the file
   actually holds.
2. **Loops `read_at` until filled or a read yields zero.** It calls
   `read_at(id, StreamId::Default, offset + filled, &mut buf[filled..])` in a
   loop, advancing `filled` by each return, stopping on a `0`-byte read
   (short-read-safe), then truncates the buffer to the bytes actually read.
3. **Rejects non-readable inodes loudly.** A `read` on a directory inode returns
   `VfsError::Unsupported` (POSIX `EISDIR`); an unknown inode errors via the
   resolver (ADR-0002); a `read_link` on a non-symlink returns `EINVAL`, and a
   symlink target read is itself capped at `LINK_CAP = 64 KiB` so a hostile link
   cannot allocate unbounded.

## Consequences

- Multi-chunk files read byte-identically: `read_whole_multichunk_file_is_byte_identical`
  drives a 200 000-byte file (> one 64 KiB chunk) through the fill loop and
  compares against ground truth; `read_mid_file_window_is_correct` verifies an
  unaligned mid-file window; `read_past_eof_is_short_and_capped` confirms the cap.
- The allocation is bounded by the artifact, not by attacker-supplied arguments —
  an alloc-bomb via a huge `size` is structurally impossible.
- Every non-read path (directory, unknown inode, non-symlink) fails loud with a
  named error rather than a panic or a misleading empty buffer
  (`unknown_inode_errors_without_panic`, `read_on_directory_errors_cleanly`,
  `read_link_on_non_symlink_errors_cleanly`).
- The `buf.get_mut(filled..)` guard inside the loop is provably unreachable under
  the `while filled < buf.len()` invariant and is annotated `// cov:unreachable`
  rather than deleted, preserving the defensive arm.
