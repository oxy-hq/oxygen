//! Archives are built here, entry by entry, so each test states exactly what a
//! hostile or oversized commit would contain.

use std::fs;
use std::io::Write;
use std::path::Path;

use flate2::Compression;
use flate2::write::GzEncoder;
use tar::{Builder, EntryType, Header};

use super::{Limits, UnpackError, Unpacked, unpack_commit_tarball};

/// The directory GitHub wraps a commit archive in.
const TOP: &str = "acme-analytics-0123abc";

enum Entry<'a> {
    File(&'a str, &'a [u8]),
    Dir(&'a str),
    Symlink(&'a str, &'a str),
    /// A regular file whose name is written into the header verbatim —
    /// `Builder` refuses to write an absolute or `..` path itself.
    RawName(&'a str, &'a [u8]),
    /// The pax global header GitHub opens every commit archive with.
    PaxGlobal,
}

fn header(kind: EntryType, size: usize) -> Header {
    let mut h = Header::new_gnu();
    h.set_entry_type(kind);
    h.set_size(size as u64);
    h.set_mode(0o644);
    h
}

fn archive(entries: &[Entry<'_>]) -> Vec<u8> {
    let mut tar = Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for entry in entries {
        match entry {
            Entry::File(path, body) => {
                let mut h = header(EntryType::Regular, body.len());
                tar.append_data(&mut h, path, *body).unwrap();
            }
            Entry::Dir(path) => {
                let mut h = header(EntryType::Directory, 0);
                tar.append_data(&mut h, path, &[][..]).unwrap();
            }
            Entry::Symlink(path, to) => {
                let mut h = header(EntryType::Symlink, 0);
                tar.append_link(&mut h, path, to).unwrap();
            }
            Entry::RawName(name, body) => {
                let mut h = header(EntryType::Regular, body.len());
                h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
                h.set_cksum();
                tar.append(&h, *body).unwrap();
            }
            Entry::PaxGlobal => {
                let body = b"52 comment=0123abc0123abc0123abc0123abc0123abc0123a\n";
                let mut h = header(EntryType::XGlobalHeader, body.len());
                tar.append_data(&mut h, "pax_global_header", &body[..])
                    .unwrap();
            }
        }
    }
    let mut gz = tar.into_inner().unwrap();
    gz.flush().unwrap();
    gz.finish().unwrap()
}

fn in_top(path: &str) -> String {
    format!("{TOP}/{path}")
}

/// Unpack into `<tmp>/tree`, so "outside the tree" is a real directory the
/// test owns and can inspect.
fn unpack(bytes: &[u8], limits: Limits) -> (tempfile::TempDir, Result<Unpacked, UnpackError>) {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("tree");
    fs::create_dir(&dest).unwrap();
    let result = unpack_commit_tarball(bytes, &dest, limits);
    (tmp, result)
}

fn is_absent(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err()
}

#[test]
fn the_top_level_directory_is_stripped() {
    let bytes = archive(&[
        Entry::PaxGlobal,
        Entry::Dir(&in_top("")),
        Entry::File(&in_top("config.yml"), b"databases: []\n"),
        Entry::Dir(&in_top("semantics")),
        Entry::File(&in_top("semantics/orders.view.yml"), b"name: orders\n"),
    ]);
    let (tmp, result) = unpack(&bytes, Limits::PRODUCTION);
    let tree = tmp.path().join("tree");

    assert_eq!(
        result,
        Ok(Unpacked {
            files: 2,
            bytes: 27,
            links_skipped: 0
        })
    );
    assert_eq!(
        fs::read(tree.join("config.yml")).unwrap(),
        b"databases: []\n"
    );
    assert_eq!(
        fs::read(tree.join("semantics/orders.view.yml")).unwrap(),
        b"name: orders\n"
    );
    assert!(is_absent(&tree.join(TOP)), "the wrapper must not survive");
    assert!(is_absent(&tree.join("pax_global_header")));
}

#[test]
fn a_path_that_climbs_out_fails_the_unpack() {
    let bytes = archive(&[
        Entry::File(&in_top("config.yml"), b"ok"),
        Entry::RawName(&in_top("../../climbed.txt"), b"out"),
    ]);
    let (tmp, result) = unpack(&bytes, Limits::PRODUCTION);

    assert!(
        matches!(result, Err(UnpackError::UnsafePath(ref p)) if p.contains("..")),
        "{result:?}"
    );
    assert!(is_absent(&tmp.path().join("climbed.txt")));
}

#[test]
fn an_absolute_path_fails_the_unpack() {
    // Short on purpose: the name goes into the header's 100-byte field.
    let bytes = archive(&[Entry::RawName("/oxy-unpack-test/absolute.txt", b"out")]);
    let (_tmp, result) = unpack(&bytes, Limits::PRODUCTION);

    assert!(
        matches!(result, Err(UnpackError::UnsafePath(ref p)) if p.starts_with('/')),
        "{result:?}"
    );
    assert!(is_absent(Path::new("/oxy-unpack-test")));
}

#[cfg(unix)]
#[test]
fn a_symlink_that_escapes_is_left_out_and_one_inside_is_kept() {
    let outside = tempfile::tempdir().unwrap();
    let absolute = outside.path().display().to_string();
    let bytes = archive(&[
        Entry::Symlink(&in_top("abs"), &absolute),
        Entry::Symlink(&in_top("up"), "../"),
        Entry::Symlink(&in_top("dangling"), "nowhere.txt"),
        Entry::Symlink(&in_top("alias.yml"), "config.yml"),
        Entry::File(&in_top("config.yml"), b"databases: []\n"),
    ]);
    let (tmp, result) = unpack(&bytes, Limits::PRODUCTION);
    let tree = tmp.path().join("tree");

    assert_eq!(result.map(|u| u.links_skipped), Ok(3));
    for escaped in ["abs", "up", "dangling"] {
        assert!(is_absent(&tree.join(escaped)), "{escaped} must not exist");
    }
    assert_eq!(
        fs::read(tree.join("alias.yml")).unwrap(),
        b"databases: []\n"
    );
}

/// Each link's target string stays inside the tree; resolved on disk, the
/// second walks through the first and comes out above it.
#[cfg(unix)]
#[test]
fn two_links_that_are_each_inside_cannot_escape_together() {
    let bytes = archive(&[
        Entry::Dir(&in_top("q")),
        Entry::Dir(&in_top("x")),
        Entry::Symlink(&in_top("q/root"), ".."),
        Entry::Symlink(&in_top("x/out"), "../q/root/.."),
    ]);
    let (tmp, result) = unpack(&bytes, Limits::PRODUCTION);
    let tree = tmp.path().join("tree");

    assert_eq!(result.map(|u| u.links_skipped), Ok(1));
    assert!(tree.join("q/root").exists(), "the first link is harmless");
    assert!(is_absent(&tree.join("x/out")));
}

/// The link comes first in the archive and names a directory outside; the
/// file after it is addressed through that name.
#[cfg(unix)]
#[test]
fn a_file_is_never_written_through_a_link() {
    let outside = tempfile::tempdir().unwrap();
    let absolute = outside.path().display().to_string();
    let bytes = archive(&[
        Entry::Symlink(&in_top("out"), &absolute),
        Entry::File(&in_top("out/written.txt"), b"data"),
    ]);
    let (tmp, result) = unpack(&bytes, Limits::PRODUCTION);
    let tree = tmp.path().join("tree");

    assert!(result.is_ok(), "{result:?}");
    assert!(is_absent(&outside.path().join("written.txt")));
    assert!(fs::symlink_metadata(tree.join("out")).unwrap().is_dir());
    assert_eq!(fs::read(tree.join("out/written.txt")).unwrap(), b"data");
}

#[test]
fn a_tree_over_the_byte_limit_fails_and_is_not_truncated() {
    let limits = Limits {
        max_files: 10,
        max_bytes: 10,
    };
    let fits = archive(&[
        Entry::File(&in_top("a"), b"12345"),
        Entry::File(&in_top("b"), b"67890"),
    ]);
    assert_eq!(unpack(&fits, limits).1.map(|u| u.bytes), Ok(10));

    let over = archive(&[
        Entry::File(&in_top("a"), b"12345"),
        Entry::File(&in_top("b"), b"678901"),
    ]);
    assert_eq!(
        unpack(&over, limits).1,
        Err(UnpackError::TooLarge { limit: 10 })
    );
}

#[test]
fn a_tree_over_the_file_limit_fails() {
    let limits = Limits {
        max_files: 2,
        max_bytes: 1024,
    };
    let fits = archive(&[
        Entry::File(&in_top("a"), b"1"),
        Entry::File(&in_top("b"), b"2"),
    ]);
    assert_eq!(unpack(&fits, limits).1.map(|u| u.files), Ok(2));

    let over = archive(&[
        Entry::File(&in_top("a"), b"1"),
        Entry::File(&in_top("b"), b"2"),
        Entry::File(&in_top("c"), b"3"),
    ]);
    assert_eq!(
        unpack(&over, limits).1,
        Err(UnpackError::TooManyFiles { limit: 2 })
    );
}

/// The limit is on inodes, not on regular files: a commit of nothing but
/// directories and links is tiny compressed and must still meet it.
#[cfg(unix)]
#[test]
fn directories_and_links_count_towards_the_entry_limit() {
    let limits = Limits {
        max_files: 3,
        max_bytes: 1024,
    };
    let fits = archive(&[
        Entry::Dir(&in_top("a")),
        Entry::File(&in_top("a/config.yml"), b"x"),
        Entry::Symlink(&in_top("alias.yml"), "a/config.yml"),
    ]);
    assert_eq!(unpack(&fits, limits).1.map(|u| u.files), Ok(1));

    let over = archive(&[
        Entry::Dir(&in_top("a")),
        Entry::Dir(&in_top("b")),
        Entry::Symlink(&in_top("one"), "a"),
        Entry::Symlink(&in_top("two"), "b"),
    ]);
    assert_eq!(
        unpack(&over, limits).1,
        Err(UnpackError::TooManyFiles { limit: 3 })
    );
}

#[test]
fn bytes_that_are_not_a_tarball_are_malformed() {
    let (_tmp, result) = unpack(b"<html>Not Found</html>", Limits::PRODUCTION);
    assert!(
        matches!(result, Err(UnpackError::Malformed(_))),
        "{result:?}"
    );
}
