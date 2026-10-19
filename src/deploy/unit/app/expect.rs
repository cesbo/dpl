use std::{
    fs,
    io::{
        self,
        Read,
    },
    path::{
        Component,
        Path,
        PathBuf,
    },
};

use serde::{
    Deserialize,
    Deserializer,
    Serialize,
    de,
};
use thiserror::Error;

/// Cap on the file [`ExpectConfig`] reads. The needle is a short literal; a
/// larger file is the wrong target (a lockfile, a tarball) and is not scanned.
const EXPECT_MAX_BYTES: u64 = 1024 * 1024;

/// A file in the extracted archive that must contain a literal substring.
///
/// `path` is relative to the extracted tree (a single wrapping folder is
/// already flattened). `contains` is matched as bytes, not as a regex or as
/// JSON.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExpectConfig {
    #[serde(deserialize_with = "deserialize_archive_path")]
    pub path: String,
    #[serde(deserialize_with = "deserialize_expect_needle")]
    pub contains: String,
}

#[derive(Debug, Error)]
pub enum ExpectError {
    #[error("{path} is not in the archive")]
    Missing { path: String },

    #[error("{path} is a symlink")]
    Symlink { path: String },

    #[error("{path} is not a regular file")]
    NotFile { path: String },

    #[error("{path} resolves outside the archive")]
    Escapes { path: String },

    #[error("{path} is {len} bytes, over the {max} byte limit")]
    TooLarge { path: String, len: u64, max: u64 },

    #[error("{path} does not contain `{needle}`")]
    Mismatch { path: String, needle: String },

    #[error("contains must not be empty")]
    Empty,

    #[error("read {path}")]
    Read {
        path: String,
        #[source]
        source: io::Error,
    },
}

impl ExpectConfig {
    /// Confirm `path` is a regular file under `app_dir` whose bytes contain
    /// `contains`. The named path must itself be a file: a symlink is rejected
    /// even when its target stays inside the tree. An intermediate symlink is
    /// allowed only when the canonical path remains inside `app_dir`.
    pub fn check(&self, app_dir: &Path) -> Result<(), ExpectError> {
        let needle = self.contains.as_bytes();
        if needle.is_empty() {
            return Err(ExpectError::Empty);
        }

        let path = app_dir.join(&self.path);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(ExpectError::Missing {
                    path: self.path.clone(),
                });
            }
            Err(source) => {
                return Err(ExpectError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        let file_type = meta.file_type();
        if file_type.is_symlink() {
            return Err(ExpectError::Symlink {
                path: self.path.clone(),
            });
        }
        if !file_type.is_file() {
            return Err(ExpectError::NotFile {
                path: self.path.clone(),
            });
        }
        if meta.len() > EXPECT_MAX_BYTES {
            return Err(ExpectError::TooLarge {
                path: self.path.clone(),
                len: meta.len(),
                max: EXPECT_MAX_BYTES,
            });
        }

        // `symlink_metadata` does not follow the final component, but it does
        // traverse intermediate symlinks. Reject a path that leaves the tree.
        let root = fs::canonicalize(app_dir).map_err(|source| ExpectError::Read {
            path: self.path.clone(),
            source,
        })?;
        let canonical = fs::canonicalize(&path).map_err(|source| ExpectError::Read {
            path: self.path.clone(),
            source,
        })?;
        if !canonical.starts_with(&root) {
            return Err(ExpectError::Escapes {
                path: self.path.clone(),
            });
        }

        let mut bytes = Vec::new();
        fs::File::open(&canonical)
            .map_err(|source| ExpectError::Read {
                path: self.path.clone(),
                source,
            })?
            .take(EXPECT_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| ExpectError::Read {
                path: self.path.clone(),
                source,
            })?;
        if bytes.len() as u64 > EXPECT_MAX_BYTES {
            return Err(ExpectError::TooLarge {
                path: self.path.clone(),
                len: bytes.len() as u64,
                max: EXPECT_MAX_BYTES,
            });
        }
        if !contains_bytes(&bytes, needle) {
            return Err(ExpectError::Mismatch {
                path: self.path.clone(),
                needle: self.contains.clone(),
            });
        }
        Ok(())
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

/// Same shape as `archive::sanitize_path`: relative, no `..`, `.` dropped.
/// Stored normalized so a `./` prefix cannot disagree with the file we open.
fn deserialize_archive_path<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    archive_relative_path(&value).map_err(de::Error::custom)
}

fn archive_relative_path(value: &str) -> Result<String, &'static str> {
    if value.is_empty() {
        return Err("path must not be empty");
    }
    if value.contains('\0') {
        return Err("path must not contain NUL");
    }

    let path = Path::new(value);
    if path.is_absolute() {
        return Err("path must be relative");
    }

    let mut sanitized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => sanitized.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err("path must not contain '..'"),
            Component::RootDir | Component::Prefix(_) => return Err("path must be relative"),
        }
    }
    if sanitized.as_os_str().is_empty() {
        return Err("path must not be empty");
    }

    sanitized
        .to_str()
        .map(str::to_owned)
        .ok_or("path must be valid UTF-8")
}

fn deserialize_expect_needle<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(de::Error::custom("contains must not be empty"));
    }
    if value.len() as u64 > EXPECT_MAX_BYTES {
        return Err(de::Error::custom(format!(
            "contains exceeds the {EXPECT_MAX_BYTES} byte limit"
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    use super::*;

    fn parsed(yaml: &str) -> ExpectConfig {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn parse_err(yaml: &str) -> String {
        serde_yaml::from_str::<ExpectConfig>(yaml)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn parse_normalizes_dot_and_keeps_nested_path() {
        let config = parsed("path: ./package.json\ncontains: '\"name\": \"foo-app\"'\n");
        assert_eq!(config.path, "package.json");
        assert_eq!(config.contains, "\"name\": \"foo-app\"");

        let nested = parsed("path: apps/./web/package.json\ncontains: foo-app\n");
        assert_eq!(nested.path, "apps/web/package.json");
    }

    #[test]
    fn parse_rejects_empty_absolute_parent_and_unknown() {
        assert!(
            parse_err("path: package.json\ncontains: ''\n").contains("contains must not be empty")
        );
        assert!(parse_err("path: ''\ncontains: foo\n").contains("path must not be empty"));
        assert!(parse_err("path: .\ncontains: foo\n").contains("path must not be empty"));
        assert!(parse_err("path: /package.json\ncontains: foo\n").contains("path must be relative"));
        assert!(
            parse_err("path: apps/../../package.json\ncontains: foo\n")
                .contains("path must not contain '..'")
        );
        assert!(parse_err("path: package.json\ncontains: foo\nequals: foo\n").contains("unknown"));
        assert!(parse_err("path: package.json\n").contains("missing field"));
        assert_eq!(
            archive_relative_path("pkg\0.json"),
            Err("path must not contain NUL")
        );
    }

    #[test]
    fn parse_rejects_needle_over_limit() {
        let mut yaml = String::from("path: package.json\ncontains: ");
        yaml.push_str(&"a".repeat(EXPECT_MAX_BYTES as usize + 1));
        yaml.push('\n');
        let err = parse_err(&yaml);
        assert!(err.contains("exceeds"), "{err}");
    }

    fn tree() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn check_matches_literal_bytes() {
        let dir = tree();
        fs::write(
            dir.path().join("package.json"),
            "{\n  \"name\": \"foo-app\",\n  \"private\": true\n}\n",
        )
        .unwrap();

        parsed("path: package.json\ncontains: '\"name\": \"foo-app\"'\n")
            .check(dir.path())
            .unwrap();

        // A different name that shares a prefix must not match a delimited needle.
        fs::write(
            dir.path().join("package.json"),
            "{\n  \"name\": \"foo-admin\"\n}\n",
        )
        .unwrap();
        let err = parsed("path: package.json\ncontains: '\"name\": \"foo-app\"'\n")
            .check(dir.path())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "package.json does not contain `\"name\": \"foo-app\"`"
        );

        // `contains` is a substring: a short needle matches a longer token.
        parsed("path: package.json\ncontains: foo\n")
            .check(dir.path())
            .unwrap();
    }

    #[test]
    fn check_matches_nested_path() {
        let dir = tree();
        fs::create_dir_all(dir.path().join("apps/web")).unwrap();
        fs::write(
            dir.path().join("apps/web/package.json"),
            "name = \"foo-app\"\n",
        )
        .unwrap();

        parsed("path: apps/web/package.json\ncontains: 'name = \"foo-app\"'\n")
            .check(dir.path())
            .unwrap();
    }

    #[test]
    fn check_rejects_missing_directory_and_symlink() {
        let dir = tree();
        let expect = parsed("path: package.json\ncontains: foo-app\n");

        let err = expect.check(dir.path()).unwrap_err();
        assert!(matches!(err, ExpectError::Missing { .. }), "{err}");

        fs::create_dir(dir.path().join("package.json")).unwrap();
        let err = expect.check(dir.path()).unwrap_err();
        assert!(matches!(err, ExpectError::NotFile { .. }), "{err}");
        fs::remove_dir(dir.path().join("package.json")).unwrap();

        // The target contains the needle; the named path is still a symlink.
        fs::write(dir.path().join("real.json"), "foo-app").unwrap();
        symlink("real.json", dir.path().join("package.json")).unwrap();
        let err = expect.check(dir.path()).unwrap_err();
        assert!(matches!(err, ExpectError::Symlink { .. }), "{err}");
    }

    #[test]
    fn check_rejects_file_over_limit_even_when_it_contains_the_needle() {
        let dir = tree();
        let path = dir.path().join("package.json");
        fs::write(&path, "foo-app").unwrap();
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(EXPECT_MAX_BYTES + 1).unwrap();

        let err = parsed("path: package.json\ncontains: foo-app\n")
            .check(dir.path())
            .unwrap_err();
        assert!(
            matches!(
                err,
                ExpectError::TooLarge { len, max, .. }
                    if len == EXPECT_MAX_BYTES + 1 && max == EXPECT_MAX_BYTES
            ),
            "{err}"
        );
    }

    #[test]
    fn check_allows_intermediate_symlink_that_stays_inside() {
        let dir = tree();
        fs::create_dir(dir.path().join("real")).unwrap();
        fs::write(dir.path().join("real/package.json"), "foo-app").unwrap();
        symlink("real", dir.path().join("link")).unwrap();

        parsed("path: link/package.json\ncontains: foo-app\n")
            .check(dir.path())
            .unwrap();
    }

    #[test]
    fn check_rejects_intermediate_symlink_that_leaves_the_tree() {
        let root = tree();
        let app = root.path().join("app");
        let outside = root.path().join("outside");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret"), "foo-app").unwrap();
        symlink(&outside, app.join("link")).unwrap();

        let err = parsed("path: link/secret\ncontains: foo-app\n")
            .check(&app)
            .unwrap_err();
        assert!(matches!(err, ExpectError::Escapes { .. }), "{err}");
    }

    #[test]
    fn check_empty_needle_does_not_panic() {
        let dir = tree();
        fs::write(dir.path().join("package.json"), "foo").unwrap();
        let expect = ExpectConfig {
            path: "package.json".into(),
            contains: String::new(),
        };
        assert!(matches!(
            expect.check(dir.path()).unwrap_err(),
            ExpectError::Empty
        ));
    }
}
