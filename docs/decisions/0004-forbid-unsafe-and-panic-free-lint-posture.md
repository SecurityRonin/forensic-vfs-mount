# 4. `forbid(unsafe)` and a panic-free lint posture

Date: 2026-07-24
Status: Accepted

## Context

The constitution's Paranoid Gatekeeper standard requires every crate on the
evidence path to never panic, never read out of bounds, and never trust a value
from the image. Its `unsafe` policy is graded by need: `forbid(unsafe_code)` is
the default and the goal — a provable, badge-able "zero places a crafted input
can corrupt memory" — and is downgraded to `deny` + a bounded per-site `#[allow]`
**only** where a real benefit (e.g. an `mmap`) justifies surrendering the
compiler's memory-safety guarantee.

`forensic-vfs-mount` is a pure adapter: it holds two `HashMap`s and a counter,
loops a trait method, and copies bytes into a `Vec`. It performs no `mmap`, no
FFI, and no pointer arithmetic — there is no benefit that would justify any
`unsafe` at all.

## Decision

- Set `unsafe_code = "forbid"` in `[lints.rust]` (`Cargo.toml`) and repeat it as
  `#![forbid(unsafe_code)]` in `src/lib.rs`. Because the crate needs no `unsafe`,
  it takes the strong `forbid` (which cannot be locally overridden), not the
  `deny` + allow exception the mmap readers use.
- Adopt the panic-free lint tier: `unwrap_used = "deny"` and `expect_used =
  "deny"` in `[lints.clippy]`, alongside the base `all`/`pedantic` warn groups
  and the standard pragmatic allows (`module_name_repetitions`,
  `must_use_candidate`, `missing_errors_doc`, `missing_panics_doc`).
- Back the lints with behaviour: every fallible path returns a `VfsResult` with a
  loud, named `VfsError`, never a panic. The mutex guard recovers from poisoning
  rather than propagating a panic (ADR-0002); an unknown inode, an `EISDIR`, and
  an `EINVAL` are all errors, not panics.

## Consequences

- The crate qualifies for the `unsafe forbidden` trust badge — memory safety is
  proved by the compiler, not asserted.
- `unwrap`/`expect` are compile errors in production code; the integration tests
  opt back in with `#![allow(clippy::unwrap_used, clippy::expect_used)]`
  (`tests/mount.rs`), the standard test carve-out.
- Because the crate parses no untrusted binary structure of its own (it only
  drives a trait it does not implement), it takes the panic-free static posture
  without a dedicated `cargo-fuzz` target — the untrusted-input fuzzing burden
  lives in the format readers beneath `forensic-vfs`, not in this adapter.
