use std::{
    fs::{
        self,
        File,
    },
    io,
    path::{
        Component,
        Path,
        PathBuf,
    },
};

use cuid::cuid2;
use flate2::read::GzDecoder;
use tar::EntryType;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("open archive")]
    Open(#[source] io::Error),
    #[error("create directory")]
    CreateDir(#[source] io::Error),
    #[error("create file")]
    CreateFile(#[source] io::Error),
    #[error("flatten directory")]
    Flatten(#[source] io::Error),
    #[error("read entry")]
    ReadEntry(#[source] io::Error),
    #[error("empty archive")]
    EmptyArchive,
}

/// Extracts a `.tar.gz` archive into `dst` and flattens a single top-level directory
/// if the archive was packed with a wrapping folder (e.g. `myapp/` → contents moved to `dst`).
pub fn extract(archive_path: &Path, dst: &Path) -> Result<(), ArchiveError> {
    extract_tar_gz(archive_path, dst)?;
    flatten_top_level(dst)?;
    Ok(())
}

fn extract_tar_gz(archive_path: &Path, dst: &Path) -> Result<(), ArchiveError> {
    let file = File::open(archive_path).map_err(ArchiveError::Open)?;

    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    fs::create_dir_all(dst).map_err(ArchiveError::CreateDir)?;
    let dst = dst.canonicalize().map_err(ArchiveError::CreateDir)?;

    let entries = archive.entries().map_err(ArchiveError::ReadEntry)?;
    for entry in entries {
        let mut entry = entry.map_err(ArchiveError::ReadEntry)?;

        let entry_path = entry.path().map_err(ArchiveError::ReadEntry)?.into_owned();
        let entry_path = sanitize_path(&entry_path).map_err(ArchiveError::ReadEntry)?;
        let output_path = dst.join(&entry_path);

        let entry_type = entry.header().entry_type();

        match entry_type {
            EntryType::Directory => {
                fs::create_dir_all(&output_path).map_err(ArchiveError::CreateDir)?;
            }
            EntryType::Regular | EntryType::Symlink => {
                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent).map_err(ArchiveError::CreateDir)?;
                }
                entry
                    .unpack(&output_path)
                    .map_err(ArchiveError::CreateFile)?;
            }
            EntryType::XGlobalHeader | EntryType::XHeader => {}
            _ => {
                return Err(ArchiveError::ReadEntry(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unsupported entry type {:?} {:?}", entry_type, &entry_path),
                )));
            }
        }
    }

    Ok(())
}

/// If `dir` contains exactly one entry and it is a directory, replaces `dir`
/// with the contents of that subdirectory. Does nothing otherwise.
fn flatten_top_level(dir: &Path) -> Result<(), ArchiveError> {
    let mut entries = fs::read_dir(dir).map_err(ArchiveError::CreateDir)?;

    let first = match entries.next() {
        Some(entry) => entry.map_err(|_| ArchiveError::EmptyArchive)?,
        None => return Ok(()),
    };

    if entries.next().is_some() {
        return Ok(());
    }

    if !first.path().is_dir() {
        return Ok(());
    }

    let nested = first.path();
    let parent = dir.parent().ok_or_else(|| {
        ArchiveError::Flatten(io::Error::new(io::ErrorKind::InvalidInput, "no parent dir"))
    })?;

    let tmp = parent.join(format!(".flatten_{}", cuid2()));

    fs::rename(&nested, &tmp).map_err(ArchiveError::Flatten)?;
    fs::remove_dir(dir).map_err(ArchiveError::Flatten)?;
    fs::rename(&tmp, dir).map_err(ArchiveError::Flatten)?;

    Ok(())
}

fn sanitize_path(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "absolute path not allowed",
        ));
    }

    let mut sanitized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Normal(part) => sanitized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid path component",
                ));
            }
        }
    }

    Ok(sanitized)
}

#[cfg(test)]
mod tests {
    use flate2::{
        Compression,
        write::GzEncoder,
    };
    use tempfile::TempDir;

    use super::*;

    fn create_tar_gz(entries: &[(&str, Option<&[u8]>)]) -> (PathBuf, TempDir) {
        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("test.tar.gz");
        let file = File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(file, Compression::fast());
        let mut builder = tar::Builder::new(encoder);

        for &(name, data) in entries {
            match data {
                Some(content) => {
                    let mut header = tar::Header::new_gnu();
                    header.set_path(name).unwrap();
                    header.set_size(content.len() as u64);
                    header.set_mode(0o644);
                    header.set_entry_type(EntryType::Regular);
                    header.set_cksum();
                    builder.append(&header, content).unwrap();
                }
                None => {
                    let mut header = tar::Header::new_gnu();
                    header.set_path(name).unwrap();
                    header.set_size(0);
                    header.set_mode(0o755);
                    header.set_entry_type(EntryType::Directory);
                    header.set_cksum();
                    builder.append(&header, &[][..]).unwrap();
                }
            }
        }

        builder.into_inner().unwrap().finish().unwrap();
        (archive_path, tmp)
    }

    fn create_tar_gz_with_symlink(file_name: &str, link_target: &str) -> (PathBuf, TempDir) {
        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("test.tar.gz");
        let file = File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(file, Compression::fast());
        let mut builder = tar::Builder::new(encoder);

        let mut header = tar::Header::new_gnu();
        header.set_path(file_name).unwrap();
        header.set_size(0);
        header.set_mode(0o644);
        header.set_entry_type(EntryType::Symlink);
        header.set_link_name(link_target).unwrap();
        header.set_cksum();
        builder.append(&header, &[][..]).unwrap();

        builder.into_inner().unwrap().finish().unwrap();
        (archive_path, tmp)
    }

    fn create_tar_gz_with_hardlink(file_name: &str, link_target: &str) -> (PathBuf, TempDir) {
        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("test.tar.gz");
        let file = File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(file, Compression::fast());
        let mut builder = tar::Builder::new(encoder);

        let mut header = tar::Header::new_gnu();
        header.set_path(link_target).unwrap();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_entry_type(EntryType::Regular);
        header.set_cksum();
        builder.append(&header, b"hello" as &[u8]).unwrap();

        let mut header = tar::Header::new_gnu();
        header.set_path(file_name).unwrap();
        header.set_size(0);
        header.set_mode(0o644);
        header.set_entry_type(EntryType::Link);
        header.set_link_name(link_target).unwrap();
        header.set_cksum();
        builder.append(&header, &[][..]).unwrap();

        builder.into_inner().unwrap().finish().unwrap();
        (archive_path, tmp)
    }

    // Write raw bytes into the tar header name field to bypass
    // the tar crate's built-in path validation.
    fn write_raw_path(header: &mut tar::Header, path: &[u8]) {
        let name_field = &mut header.as_gnu_mut().unwrap().name;
        let len = path.len().min(name_field.len());
        name_field[.. len].copy_from_slice(&path[.. len]);
        if len < name_field.len() {
            name_field[len] = 0;
        }
    }

    #[test]
    fn extract_valid_archive() {
        let (archive, _dir) = create_tar_gz(&[
            ("hello.txt", Some(b"hello world")),
            ("subdir/nested.txt", Some(b"nested content")),
        ]);

        let target = TempDir::new().unwrap();
        extract_tar_gz(&archive, target.path()).unwrap();

        assert_eq!(
            fs::read_to_string(target.path().join("hello.txt")).unwrap(),
            "hello world",
        );
        assert_eq!(
            fs::read_to_string(target.path().join("subdir/nested.txt")).unwrap(),
            "nested content",
        );
    }

    #[test]
    fn extract_with_directories() {
        let (archive, _dir) =
            create_tar_gz(&[("mydir/", None), ("mydir/file.txt", Some(b"content"))]);

        let target = TempDir::new().unwrap();
        extract_tar_gz(&archive, target.path()).unwrap();

        assert!(target.path().join("mydir").is_dir());
        assert_eq!(
            fs::read_to_string(target.path().join("mydir/file.txt")).unwrap(),
            "content",
        );
    }

    #[test]
    fn extract_with_symlink() {
        let name = "passwd.txt";
        let link = "/etc/passwd";
        let (archive, _dir) = create_tar_gz_with_symlink(name, link);

        let target = TempDir::new().unwrap();
        extract_tar_gz(&archive, target.path()).unwrap();

        let path = target.path().join(name);
        assert!(path.is_symlink());
        assert_eq!(fs::read_link(path).unwrap(), PathBuf::from(link));
    }

    #[test]
    fn reject_path_traversal() {
        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("test.tar.gz");
        let file = File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(file, Compression::fast());
        let mut builder = tar::Builder::new(encoder);

        let mut header = tar::Header::new_gnu();
        write_raw_path(&mut header, b"../escape.txt");
        header.set_size(3);
        header.set_mode(0o644);
        header.set_entry_type(EntryType::Regular);
        header.set_cksum();
        builder.append(&header, b"bad" as &[u8]).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let target = TempDir::new().unwrap();
        let err = extract_tar_gz(&archive_path, target.path()).unwrap_err();

        assert!(matches!(err, ArchiveError::ReadEntry { .. }));
    }

    #[test]
    fn reject_absolute_path() {
        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("test.tar.gz");
        let file = File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(file, Compression::fast());
        let mut builder = tar::Builder::new(encoder);

        let mut header = tar::Header::new_gnu();
        write_raw_path(&mut header, b"/etc/passwd");
        header.set_size(4);
        header.set_mode(0o644);
        header.set_entry_type(EntryType::Regular);
        header.set_cksum();
        builder.append(&header, b"root" as &[u8]).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let target = TempDir::new().unwrap();
        let err = extract_tar_gz(&archive_path, target.path()).unwrap_err();

        assert!(matches!(err, ArchiveError::ReadEntry { .. }));
    }

    #[test]
    fn reject_hardlink() {
        let (archive, _dir) = create_tar_gz_with_hardlink("link.txt", "original.txt");

        let target = TempDir::new().unwrap();
        let err = extract_tar_gz(&archive, target.path()).unwrap_err();

        assert!(matches!(err, ArchiveError::ReadEntry { .. }));
    }

    #[test]
    fn create_target_dir_if_missing() {
        let (archive, _dir) = create_tar_gz(&[("file.txt", Some(b"data"))]);

        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nonexistent/subdir");
        extract_tar_gz(&archive, &target).unwrap();

        assert_eq!(fs::read_to_string(target.join("file.txt")).unwrap(), "data",);
    }

    #[test]
    fn reject_nested_traversal() {
        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("test.tar.gz");
        let file = File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(file, Compression::fast());
        let mut builder = tar::Builder::new(encoder);

        let mut header = tar::Header::new_gnu();
        write_raw_path(&mut header, b"a/b/../../c/../../../escape.txt");
        header.set_size(3);
        header.set_mode(0o644);
        header.set_entry_type(EntryType::Regular);
        header.set_cksum();
        builder.append(&header, b"bad" as &[u8]).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let target = TempDir::new().unwrap();
        let err = extract_tar_gz(&archive_path, target.path()).unwrap_err();

        assert!(matches!(err, ArchiveError::ReadEntry { .. }));
    }

    #[test]
    fn open_nonexistent_archive() {
        let target = TempDir::new().unwrap();
        let err = extract_tar_gz(Path::new("/tmp/nonexistent.tar.gz"), target.path()).unwrap_err();

        assert!(matches!(err, ArchiveError::Open { .. }));
    }

    #[test]
    fn flatten_nested_single_dir() {
        let (archive, _dir) = create_tar_gz(&[
            ("myapp/", None),
            ("myapp/index.js", Some(b"console.log('hi')")),
            ("myapp/package.json", Some(b"{}")),
        ]);

        let target = TempDir::new().unwrap();
        extract(&archive, target.path()).unwrap();

        assert!(!target.path().join("myapp").exists());
        assert_eq!(
            fs::read_to_string(target.path().join("index.js")).unwrap(),
            "console.log('hi')",
        );
        assert_eq!(
            fs::read_to_string(target.path().join("package.json")).unwrap(),
            "{}",
        );
    }

    #[test]
    fn no_flatten_multiple_entries() {
        let (archive, _dir) = create_tar_gz(&[("a.txt", Some(b"a")), ("b.txt", Some(b"b"))]);

        let target = TempDir::new().unwrap();
        extract(&archive, target.path()).unwrap();

        assert!(target.path().join("a.txt").exists());
        assert!(target.path().join("b.txt").exists());
    }

    #[test]
    fn no_flatten_single_file() {
        let (archive, _dir) = create_tar_gz(&[("only.txt", Some(b"content"))]);

        let target = TempDir::new().unwrap();
        extract(&archive, target.path()).unwrap();

        assert_eq!(
            fs::read_to_string(target.path().join("only.txt")).unwrap(),
            "content",
        );
    }

    #[test]
    fn flatten_preserves_nested_subdirs() {
        let (archive, _dir) = create_tar_gz(&[
            ("project/", None),
            ("project/Cargo.toml", Some(b"[package]")),
            ("project/src/", None),
            ("project/src/main.rs", Some(b"fn main() {}")),
        ]);

        let target = TempDir::new().unwrap();
        extract(&archive, target.path()).unwrap();

        assert!(!target.path().join("project").exists());
        assert_eq!(
            fs::read_to_string(target.path().join("Cargo.toml")).unwrap(),
            "[package]",
        );
        assert_eq!(
            fs::read_to_string(target.path().join("src/main.rs")).unwrap(),
            "fn main() {}",
        );
    }
}
