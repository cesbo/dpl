use std::{
    fs::{
        File,
        OpenOptions,
    },
    io::{
        self,
        BufRead,
        BufReader,
        Read,
        Write,
    },
    path::{
        Path,
        PathBuf,
    },
};

/// Captures newline-delimited JSON records without any runtime log prefix.
///
/// Only lines that look like JSON objects are written: after trimming the line
/// ending, the first byte must be `{` and the last byte must be `}`.
pub struct JsonlLog {
    path: PathBuf,
    max_size: u64,
    max_files: u32,
    file: File,
    written: u64,
}

impl JsonlLog {
    pub fn open_with_limits(path: &Path, max_size: u64, max_files: u32) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let written = file.metadata()?.len();

        Ok(JsonlLog {
            path: path.to_path_buf(),
            max_size,
            max_files,
            file,
            written,
        })
    }

    /// Drain a stream to EOF, writing only JSON-object-shaped lines.
    pub fn capture(&mut self, reader: impl Read) -> io::Result<()> {
        let mut reader = BufReader::new(reader);
        let mut buf = Vec::new();

        loop {
            buf.clear();

            if reader.read_until(b'\n', &mut buf)? == 0 {
                break;
            }

            if buf.ends_with(b"\n") {
                buf.pop();
            }

            if buf.ends_with(b"\r") {
                buf.pop();
            }

            if !(buf.starts_with(b"{") && buf.ends_with(b"}")) {
                continue;
            }

            self.file.write_all(&buf)?;
            self.file.write_all(b"\n")?;
            self.written += buf.len() as u64 + 1;
            if self.written >= self.max_size {
                self.rotate()?;
            }
        }

        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        for i in (1 ..= self.max_files).rev() {
            let src = if i == 1 {
                self.path.clone()
            } else {
                rotated_path(&self.path, i - 1)
            };

            match std::fs::rename(&src, rotated_path(&self.path, i)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }

        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.written = 0;

        Ok(())
    }
}

fn rotated_path(path: &Path, i: u32) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{i}"));
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use std::fs::read_to_string;

    use super::*;

    #[test]
    fn writes_only_json_object_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let mut log = JsonlLog::open_with_limits(&path, 200 * 1024 * 1024, 1).unwrap();

        log.capture(
            &b"{\"status\":200}\nnot json\n[1]\n{\"status\":404}\r\n{\"partial\":true}"[..],
        )
        .unwrap();

        assert_eq!(
            read_to_string(&path).unwrap(),
            "{\"status\":200}\n{\"status\":404}\n{\"partial\":true}\n"
        );
    }

    #[test]
    fn rotates_past_max_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let mut log = JsonlLog::open_with_limits(&path, 40, 1).unwrap();

        log.capture(&b"{\"n\":0}\n{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n{\"n\":4}\n"[..])
            .unwrap();

        let archive = rotated_path(&path, 1);
        assert!(archive.exists(), "no rotated archive created");

        let combined = format!(
            "{}{}",
            read_to_string(&archive).unwrap(),
            read_to_string(&path).unwrap()
        );
        for i in 0 .. 5 {
            assert!(
                combined.contains(&format!("{{\"n\":{i}}}")),
                "lost line {i}"
            );
        }
    }
}
