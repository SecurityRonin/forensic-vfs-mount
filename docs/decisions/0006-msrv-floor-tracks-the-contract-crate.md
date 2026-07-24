# 6. MSRV floor 1.85, tracking the contract crate

Date: 2026-07-24
Status: Accepted

## Context

The fleet policy separates the **dev toolchain** (what you build/lint with —
pinned fleet-wide to the current stable) from the **declared MSRV**
(`rust-version`, a downstream-facing promise). Published libraries keep a low,
CI-verified MSRV so they stay broadly consumable; the number is raised only when
a crate genuinely needs a newer-Rust feature — and, for an adapter, it cannot
sensibly promise support for a Rust older than the crate it adapts.

## Decision

- Declare `rust-version = "1.85"` in `Cargo.toml`. This matches the MSRV of the
  contract crate it wraps — `forensic-vfs`'s workspace `rust-version = "1.85"`
  (`~/src/forensic-vfs/Cargo.toml`) — so the adapter promises exactly the floor
  its own dependency requires, no lower (which would be a false promise) and no
  higher (which would needlessly narrow its audience).
- Pin the **dev toolchain** separately in `rust-toolchain.toml` to the current
  fleet stable (`1.96.0`, with `clippy`/`rustfmt` components), the single source
  of truth for what contributors and CI build with.

## Consequences

- The declared MSRV is honest: a `1.85` toolchain can build the adapter because
  its only dependency also floors at `1.85`.
- The floor is a promise to verify in CI, not a value to raise casually — raising
  it is a near-breaking change for downstream consumers, taken only if a future
  edition/feature forces it.
- If `forensic-vfs` raises its own MSRV in a later release, this crate's floor
  follows it (an adapter cannot support a Rust its contract cannot), and that
  bump is recorded when it happens.
