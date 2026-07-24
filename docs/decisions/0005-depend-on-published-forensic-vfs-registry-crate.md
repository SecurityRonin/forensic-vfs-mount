# 5. Depend on the published `forensic-vfs` registry crate

Date: 2026-07-24
Status: Accepted

## Context

The adapter was first wired against a **path** dependency on the in-flight
contract crate — `forensic-vfs = { version = "0.1", path =
"../forensic-vfs/crates/core" }` — with a `[patch.crates-io]` entry redirecting
`forensic-vfs` to the same local path. The patch existed to unify the contract
crate across this crate and the `ad1-core` dev-dependency (whose `vfs` feature
pulls `forensic-vfs` from crates.io), so both resolve to the *same* `FileSystem`
trait and `Ad1Vfs` satisfies `DynFs`.

The constitution's Dependency-Preference rule is: prefer the **published**
registry crate over a `path` dependency once it is on crates.io — path deps are
for crates not yet published or a coordinated in-flight change; a registry
version is reproducible and decoupled from the local checkout layout. Once
`forensic-vfs 0.7` was published, the local path pin left the adapter behind a
stale caret and coupled to a sibling checkout path.

## Decision

Drop the path dependency and the `[patch.crates-io]` block; depend on
`forensic-vfs = "0.7"` from crates.io (commit `79f1c94`, "fix(deps): widen stale
caret requirement to published version"). With both this crate and the
`ad1-core` dev-dep now resolving `forensic-vfs` from the registry at the same
`0.7` line, the patch is no longer needed to unify the trait — the registry
version does it.

Commit `Cargo.lock` (`ae12bdc`) so CI resolves the shipped graph and `cargo vet
--locked` stays stable, per the fleet "commit Cargo.lock in every repo" rule.

## Consequences

- Builds are reproducible and independent of where `../forensic-vfs` sits on
  disk; a sibling rename or move no longer breaks this crate.
- The dependency tracks the published contract, so Renovate's `rangeStrategy:
  "bump"` (`renovate.json`) keeps it fresh — a future `forensic-vfs 0.8` arrives
  as a reviewed PR rather than sitting silently behind a caret.
- The `ad1-core` dev-dependency stays a `path` dep (it is consumed only by the
  integration tests as an independent oracle, not shipped), which is the
  legitimate remaining use of a path dep.
