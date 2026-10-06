//! Unpack a GitHub commit tarball into a directory, safely and within limits.
//!
//! The archive is the content of somebody's repository, so nothing in it is
//! trusted: a path may try to climb out of the destination, a header may lie
//! about a size, a symlink may point at the host. Three rules cover it.
//!
//! * **Paths.** An absolute path or one with a `..` component fails the whole
//!   unpack — a commit archive never contains either, so this is a malformed
//!   or hostile archive, not something to work around.
//! * **Limits.** Entries (files, directories and links alike) and total bytes
//!   are counted as they are met, never read from a header, and exceeding
//!   either is an error. Nothing is
//!   ever truncated: a compile of part of a tree would promote a revision
//!   that is missing files.
//! * **Symlinks.** Regular files are all written before any link exists, so
//!   no file is ever written *through* a link. Links are then created one at
//!   a time and each is resolved on disk: one that does not land inside the
//!   tree is removed. Checking the target string alone is not enough — two
//!   links that are each harmless can resolve outside together.
//!
//! GitHub wraps a commit archive in one top-level directory
//! (`<owner>-<repo>-<sha>/`); it is stripped.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};

use crate::server::api::custom_apps_publish::is_safe_relative_path;

/// The most entries — files, directories and links — one commit may unpack to.
///
/// The largest production workspace compiles 169 files (read 2026-10-05).
/// 20,000 leaves two orders of magnitude for the repository around it, and
/// bounds inode use on a pod that has no volume. Directories and links count
/// because they take inodes too: a commit made of empty directories or of
/// symlinks is tiny compressed and would otherwise never meet a limit.
pub const MAX_TREE_FILES: usize = 20_000;

/// The most bytes one commit may unpack to: 1 GiB.
///
/// Definitions are kilobytes; the size is committed DuckDB data. The compile's
/// S3 mirror refuses a data file over 256 MiB (`oxy-compile`'s
/// `MAX_MIRROR_FILE_BYTES`), so this holds four files at that ceiling — and it
/// is ephemeral disk on a pod with no volume, so it is not larger.
pub const MAX_TREE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_files: usize,
    pub max_bytes: u64,
}

impl Limits {
    pub const PRODUCTION: Limits = Limits {
        max_files: MAX_TREE_FILES,
        max_bytes: MAX_TREE_BYTES,
    };
}

/// What an unpack wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unpacked {
    /// Regular files written.
    pub files: usize,
    pub bytes: u64,
    /// Symlinks and hard links left out: they resolved outside the tree, or
    /// to nothing.
    pub links_skipped: usize,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UnpackError {
    #[error("the archive is not a readable gzipped tarball: {0}")]
    Malformed(String),
    #[error("the archive contains an unsafe path: {0}")]
    UnsafePath(String),
    #[error("the commit has more than {limit} files, directories and links")]
    TooManyFiles { limit: usize },
    #[error("the commit unpacks to more than {limit} bytes")]
    TooLarge { limit: u64 },
    #[error("could not write the fetched tree: {0}")]
    Io(String),
}

fn io_err(e: io::Error) -> UnpackError {
    UnpackError::Io(e.to_string())
}

/// Unpack `archive` (a gzipped tarball) into `dest`, which must exist.
///
/// Blocking: call it from `spawn_blocking`.
pub fn unpack_commit_tarball(
    archive: impl Read,
    dest: &Path,
    limits: Limits,
) -> Result<Unpacked, UnpackError> {
    let mut tar = Archive::new(GzDecoder::new(archive));
    let entries = tar
        .entries()
        .map_err(|e| UnpackError::Malformed(e.to_string()))?;
    let mut done = Unpacked {
        files: 0,
        bytes: 0,
        links_skipped: 0,
    };
    let mut links: Vec<(PathBuf, PathBuf)> = Vec::new();
    // Every entry that will take an inode, counted before anything is made of
    // it. This is also what bounds `links`.
    let mut entries_seen: usize = 0;

    for entry in entries {
        let mut entry = entry.map_err(|e| UnpackError::Malformed(e.to_string()))?;
        let kind = entry.header().entry_type();
        // GitHub opens the archive with a pax global header carrying the
        // commit id. It describes the archive, not a file in it.
        if matches!(kind, EntryType::XGlobalHeader | EntryType::XHeader) {
            continue;
        }
        let raw = entry
            .path()
            .map_err(|e| UnpackError::Malformed(e.to_string()))?
            .into_owned();
        let Some(rel) = strip_top_level(&raw)? else {
            continue;
        };
        entries_seen += 1;
        if entries_seen > limits.max_files {
            return Err(UnpackError::TooManyFiles {
                limit: limits.max_files,
            });
        }
        let target = dest.join(&rel);
        match kind {
            EntryType::Directory => fs::create_dir_all(&target).map_err(io_err)?,
            EntryType::Regular | EntryType::Continuous => {
                write_file(&mut entry, &target, limits, &mut done)?;
            }
            EntryType::Symlink => match entry.link_name() {
                Ok(Some(to)) => links.push((target, to.into_owned())),
                _ => done.links_skipped += 1,
            },
            // Hard links, devices, fifos: `git archive` writes none of them.
            _ => done.links_skipped += 1,
        }
    }

    let root = dest.canonicalize().map_err(io_err)?;
    for (link, to) in links {
        if !create_contained_symlink(&root, &link, &to) {
            done.links_skipped += 1;
        }
    }
    Ok(done)
}

/// `raw` without the archive's top-level directory, or `None` for that
/// directory itself. Errors on a path that could leave the destination.
fn strip_top_level(raw: &Path) -> Result<Option<PathBuf>, UnpackError> {
    if !is_safe_relative_path(raw) {
        return Err(UnpackError::UnsafePath(raw.display().to_string()));
    }
    let rel: PathBuf = raw
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .skip(1)
        .collect();
    Ok(Some(rel).filter(|p| !p.as_os_str().is_empty()))
}

/// Write one regular file, charging it against both limits as it goes. The
/// read is bounded by what is left of the byte budget, so a header that
/// understates its size cannot get more than one byte past it.
fn write_file(
    entry: &mut impl Read,
    target: &Path,
    limits: Limits,
    done: &mut Unpacked,
) -> Result<(), UnpackError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    let remaining = limits.max_bytes - done.bytes;
    let mut file = fs::File::create(target).map_err(io_err)?;
    let written = io::copy(&mut entry.by_ref().take(remaining + 1), &mut file)
        .map_err(|e| UnpackError::Malformed(e.to_string()))?;
    if written > remaining {
        return Err(UnpackError::TooLarge {
            limit: limits.max_bytes,
        });
    }
    done.files += 1;
    done.bytes += written;
    Ok(())
}

/// Create `link -> to` and keep it only if it resolves to something inside
/// `root`. Returns whether the link was kept.
///
/// Every regular file already exists, so a link that resolves nowhere is
/// dangling in the commit itself and is no loss. Nothing is overwritten —
/// creating over an existing name fails — so a link that passed cannot be
/// re-pointed by a later one.
#[cfg(unix)]
fn create_contained_symlink(root: &Path, link: &Path, to: &Path) -> bool {
    // The link's own directory must be inside the tree for real, not just by
    // name: a parent component may itself be a link kept earlier. Those all
    // resolve inside `root`, so creating the directory cannot leave it either.
    let Some(parent) = link.parent() else {
        return false;
    };
    let parent_inside = fs::create_dir_all(parent).is_ok()
        && parent.canonicalize().is_ok_and(|p| p.starts_with(root));
    if !parent_inside || std::os::unix::fs::symlink(to, link).is_err() {
        return false;
    }
    let inside = link.canonicalize().is_ok_and(|p| p.starts_with(root));
    if !inside {
        let _ = fs::remove_file(link);
    }
    inside
}

#[cfg(not(unix))]
fn create_contained_symlink(_root: &Path, _link: &Path, _to: &Path) -> bool {
    false
}

#[cfg(test)]
#[path = "unpack_tests.rs"]
mod tests;
