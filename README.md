# forensic-vfs-mount

**The keystone that turns any `forensic-vfs` filesystem into the surface a FUSE or Dokan handler wants.**

```rust
use forensic_vfs_mount::MountFs;

// Wrap any Arc<dyn FileSystem> once — get stable u64 inodes and whole-file reads.
let mount = MountFs::new(fs);
let root = MountFs::ROOT_INO; // 1, the FUSE convention
```

## Why this exists

The fleet's unified VFS contract (`forensic_vfs::FileSystem`) addresses files by an opaque `FileId` enum and offers only a short-read `read_at`. A FUSE or Dokan mount handler wants something different: **u64 inodes** and a **whole-file read**. `forensic-vfs-mount` is the adapter between the two, so every mount consumer stops re-deriving the same inode mapping and the same fill loop.

## What it owns

- **Stable `u64` ↔ `FileId` mapping.** FUSE requires a file to keep its inode for the life of the mount. The root `FileId` is inode `1`; every other `FileId` is interned to a fresh incrementing `u64` the first time it is seen (in `read_dir`/`lookup`) and reused thereafter. An unknown inode is a loud `VfsError`, never a panic.
- **Whole-file, short-read-safe `read`.** The contract only offers `read_at`, which may short-read. `MountFs::read` loops it until the request is filled or a read yields zero, and caps its allocation at the file's own `meta().size` so an absurd size argument cannot allocate without bound.

The crate is `#![forbid(unsafe_code)]` — pure, panic-free adapter logic over the VFS contract.

## Where it fits

`forensic-vfs-mount` sits between [forensic-vfs](https://github.com/SecurityRonin/forensic-vfs) (the VFS contract) and a mount front-end, letting a mount tool present a whole stacked image — container, volume system, crypto layer, filesystem — as one inode-addressed tree without knowing one format from another.

---

[Privacy Policy](https://securityronin.github.io/forensic-vfs-mount/privacy/) · [Terms of Service](https://securityronin.github.io/forensic-vfs-mount/terms/) · © 2026 Security Ronin Ltd
