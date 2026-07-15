//! Generic `forensic_vfs::FileSystem` -> u64-inode / whole-file mount adapter.
//!
//! [`MountFs`] is the missing keystone between the fleet's unified VFS contract
//! ([`forensic_vfs::FileSystem`], addressed by an opaque [`FileId`] enum with no
//! whole-file read) and a FUSE/Dokan handler, which wants **u64 inodes** and a
//! **whole-file `read`**. Wrapping any `Arc<dyn FileSystem>` in a `MountFs` gives
//! that surface once, so every mount consumer stops re-deriving the same inode
//! mapping and the same `read_at` fill loop.
//!
//! Two jobs the adapter owns:
//!
//! - **Stable `u64` <-> [`FileId`] mapping.** FUSE requires a given file keep its
//!   inode for the life of the mount. The root [`FileId`] is inode
//!   [`MountFs::ROOT_INO`] (`1`, the FUSE convention); every other `FileId` is
//!   interned to a fresh incrementing `u64` the first time it is seen (in
//!   [`read_dir`](MountFs::read_dir)/[`lookup`](MountFs::lookup)) and reused
//!   thereafter. An unknown inode is a loud [`VfsError`], never a panic.
//! - **Whole-file, short-read-safe `read`.** The contract only offers
//!   `read_at(&self, .., off, buf) -> usize`, which may short-read; `read` loops
//!   it until the request is filled or a read yields 0, and caps its allocation
//!   at the file's own `meta().size` so an absurd `size` argument cannot allocate
//!   without bound.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use forensic_vfs::{DynFs, FileId, FsMeta, NodeKind, StreamId, VfsError, VfsResult};

/// One directory child, in the u64-inode surface a mount handler consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirItem {
    pub name: Vec<u8>,
    pub ino: u64,
    pub kind: NodeKind,
}

/// Bidirectional `u64 <-> FileId` interning table. Behind a [`Mutex`] on the
/// [`MountFs`] so the whole adapter stays `&self` (shared across mount workers).
#[derive(Debug)]
struct InodeMap {
    forward: HashMap<FileId, u64>,
    reverse: HashMap<u64, FileId>,
    next: u64,
}

impl InodeMap {
    fn new(root: FileId) -> Self {
        let mut forward = HashMap::new();
        let mut reverse = HashMap::new();
        forward.insert(root, MountFs::ROOT_INO);
        reverse.insert(MountFs::ROOT_INO, root);
        Self {
            forward,
            reverse,
            next: MountFs::ROOT_INO + 1,
        }
    }

    /// Return the stable inode for `id`, assigning a fresh one the first time.
    fn intern(&mut self, id: FileId) -> u64 {
        if let Some(&ino) = self.forward.get(&id) {
            return ino;
        }
        let ino = self.next;
        // saturating so a pathological 2^64-entry tree stops handing out inodes
        // rather than wrapping and aliasing an existing file.
        self.next = self.next.saturating_add(1);
        self.forward.insert(id, ino);
        self.reverse.insert(ino, id);
        ino
    }

    /// Resolve a `u64` back to the `FileId` it was interned for.
    fn resolve(&self, ino: u64) -> Option<FileId> {
        self.reverse.get(&ino).copied()
    }
}

/// A u64-inode, whole-file view over any [`DynFs`], for FUSE/Dokan consumers.
pub struct MountFs {
    fs: DynFs,
    map: Mutex<InodeMap>,
}

impl MountFs {
    /// The fixed root inode. FUSE addresses the mount root as inode `1`.
    pub const ROOT_INO: u64 = 1;

    /// Cap on a symlink target read — a hostile link cannot allocate unbounded.
    const LINK_CAP: usize = 64 * 1024;

    /// Wrap a shared filesystem handle. The root [`FileId`] is pre-interned to
    /// [`ROOT_INO`](Self::ROOT_INO).
    #[must_use]
    pub fn new(fs: DynFs) -> Self {
        let root = fs.root();
        Self {
            fs,
            map: Mutex::new(InodeMap::new(root)),
        }
    }

    /// The root inode ([`ROOT_INO`](Self::ROOT_INO)).
    #[must_use]
    pub fn root_ino(&self) -> u64 {
        Self::ROOT_INO
    }

    /// Lock the inode map, recovering from a poisoned mutex rather than panicking
    /// (a panic in another worker must not wedge the mount).
    fn map(&self) -> std::sync::MutexGuard<'_, InodeMap> {
        self.map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn intern(&self, id: FileId) -> u64 {
        self.map().intern(id)
    }

    /// Resolve an inode to its `FileId`, or a loud error naming the offending
    /// inode (never a panic, never a silent empty result).
    fn resolve(&self, ino: u64) -> VfsResult<FileId> {
        self.map().resolve(ino).ok_or(VfsError::Unsupported {
            layer: "mount",
            scheme: format!("unknown inode {ino}"),
        })
    }

    /// List a directory's children with stable u64 inodes. Drains the contract's
    /// `DirStream` and interns each child's `FileId`.
    pub fn read_dir(&self, ino: u64) -> VfsResult<Vec<DirItem>> {
        let dir = self.resolve(ino)?;
        let stream = self.fs.read_dir(dir)?;
        let mut out = Vec::new();
        for entry in stream {
            let entry = entry?;
            let child_ino = self.intern(entry.id);
            out.push(DirItem {
                name: entry.name,
                ino: child_ino,
                kind: entry.kind,
            });
        }
        Ok(out)
    }

    /// Look a name up under a directory inode, returning the child's stable inode.
    pub fn lookup(&self, parent_ino: u64, name: &[u8]) -> VfsResult<Option<u64>> {
        let parent = self.resolve(parent_ino)?;
        match self.fs.lookup(parent, name)? {
            Some(id) => Ok(Some(self.intern(id))),
            None => Ok(None),
        }
    }

    /// The contract's [`FsMeta`] for an inode.
    pub fn meta(&self, ino: u64) -> VfsResult<FsMeta> {
        let id = self.resolve(ino)?;
        self.fs.meta(id)
    }

    /// Whole-file read: allocate `min(size, file_size - offset)`, then loop
    /// `read_at(FileId, StreamId::Default, offset + filled, &mut buf[filled..])`
    /// until the buffer is filled or a read yields 0 (short-read-safe), and
    /// truncate to the bytes actually read.
    pub fn read(&self, ino: u64, offset: u64, size: u64) -> VfsResult<Vec<u8>> {
        let id = self.resolve(ino)?;
        let meta = self.fs.meta(id)?;
        // POSIX EISDIR: a directory has no readable byte stream.
        if meta.kind == NodeKind::Dir {
            return Err(VfsError::Unsupported {
                layer: "mount",
                scheme: format!("read on directory inode {ino}"),
            });
        }
        // Cap the allocation at the file's own size so an absurd `size` (or a
        // read starting past EOF) never allocates beyond what the file holds.
        let remaining = meta.size.saturating_sub(offset);
        let want = size.min(remaining);
        let cap = usize::try_from(want).unwrap_or(usize::MAX);
        let mut buf = vec![0u8; cap];

        let mut filled = 0usize;
        while filled < buf.len() {
            let cur = offset.saturating_add(filled as u64);
            let Some(dst) = buf.get_mut(filled..) else {
                break; // cov:unreachable: filled < buf.len() holds by the while guard
            };
            let n = self.fs.read_at(id, StreamId::Default, cur, dst)?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        buf.truncate(filled);
        Ok(buf)
    }

    /// Read a symlink target. Errors on a non-symlink inode (POSIX EINVAL).
    pub fn read_link(&self, ino: u64) -> VfsResult<Vec<u8>> {
        let id = self.resolve(ino)?;
        let meta = self.fs.meta(id)?;
        if meta.kind != NodeKind::Symlink {
            return Err(VfsError::Unsupported {
                layer: "mount",
                scheme: format!("read_link on non-symlink inode {ino}"),
            });
        }
        self.fs.read_link(id, Self::LINK_CAP)
    }
}
