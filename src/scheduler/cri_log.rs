use std::{
    fs::{
        File,
        OpenOptions,
    },
    io::{
        self,
        BufRead,
        BufReader,
        BufWriter,
        Read,
        Write,
    },
    path::Path,
    sync::Mutex,
    thread,
};

use chrono::{
    SecondsFormat,
    Utc,
};

/// Captures a process's stdout/stderr in the CRI `k8s-file` log format - the
/// same line shape podman's own `k8s-file` log driver emits, so the file reads
/// like a container log:
///
/// ```text
/// 2026-06-05T10:11:12.123456789Z stdout F a normal line
/// 2026-06-05T10:11:12.123456790Z stderr F an error line
/// ```
///
/// Each record is `<RFC3339Nano timestamp> <stream> <tag> <message>`. The tag is
/// `F` for a full (newline-terminated) line, `P` for a partial final line with
/// no trailing newline. Records are appended, so a timer's log keeps run history.
pub struct CriLog {
    writer: Mutex<BufWriter<File>>,
}

impl CriLog {
    /// Open `path` for appending, creating parent dirs.
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(CriLog {
            writer: Mutex::new(BufWriter::new(file)),
        })
    }

    /// Drain both pipes to EOF concurrently, writing each line as a CRI record.
    /// Returns once both streams close, which happens when the process exits.
    /// Two threads are needed so a full pipe on one stream can't deadlock the other.
    pub fn capture(&self, stdout: impl Read + Send, stderr: impl Read + Send) -> io::Result<()> {
        thread::scope(|s| {
            let out = s.spawn(|| self.pump("stdout", stdout));
            let err = s.spawn(|| self.pump("stderr", stderr));
            let out = out
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("stdout pump panicked")));
            let err = err
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("stderr pump panicked")));
            out.and(err)
        })?;
        self.writer.lock().expect("cri log mutex poisoned").flush()
    }

    fn pump(&self, stream: &str, reader: impl Read) -> io::Result<()> {
        let mut reader = BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            if reader.read_until(b'\n', &mut buf)? == 0 {
                break;
            }
            let full = buf.last() == Some(&b'\n');
            if full {
                buf.pop();
            }
            let tag = if full { 'F' } else { 'P' };
            let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
            let msg = String::from_utf8_lossy(&buf);
            let mut w = self.writer.lock().expect("cri log mutex poisoned");
            writeln!(w, "{ts} {stream} {tag} {msg}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs::read_to_string;

    use super::*;

    /// Every CRI record is `<timestamp> <stream> <tag> <message>`; assert the
    /// fixed fields and hand back the message so a test can check content.
    fn assert_record<'a>(line: &'a str, stream: &str, tag: &str) -> &'a str {
        let mut f = line.splitn(4, ' ');
        let ts = f.next().unwrap();
        assert!(ts.ends_with('Z'), "timestamp not RFC3339Z: {line}");
        assert!(ts.contains('.'), "timestamp lacks nanos: {line}");
        assert_eq!(f.next().unwrap(), stream, "{line}");
        assert_eq!(f.next().unwrap(), tag, "{line}");
        f.next().unwrap_or("")
    }

    #[test]
    fn writes_full_lines_per_stream() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.log");
        let log = CriLog::open(&path).unwrap();
        log.capture(&b"out one\nout two\n"[..], &b"err one\n"[..])
            .unwrap();

        let body = read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3, "{body}");

        // Lines from a single stream keep their order; stdout/stderr may interleave.
        let out: Vec<&str> = lines
            .iter()
            .filter(|l| l.contains(" stdout "))
            .map(|l| assert_record(l, "stdout", "F"))
            .collect();
        let err: Vec<&str> = lines
            .iter()
            .filter(|l| l.contains(" stderr "))
            .map(|l| assert_record(l, "stderr", "F"))
            .collect();
        assert_eq!(out, ["out one", "out two"]);
        assert_eq!(err, ["err one"]);
    }

    #[test]
    fn final_line_without_newline_is_partial() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.log");
        let log = CriLog::open(&path).unwrap();
        log.capture(&b"done\nno newline"[..], &b""[..]).unwrap();

        let body = read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(assert_record(lines[0], "stdout", "F"), "done");
        assert_eq!(assert_record(lines[1], "stdout", "P"), "no newline");
    }

    #[test]
    fn appends_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.log");

        CriLog::open(&path)
            .unwrap()
            .capture(&b"first run\n"[..], &b""[..])
            .unwrap();
        CriLog::open(&path)
            .unwrap()
            .capture(&b"second run\n"[..], &b""[..])
            .unwrap();

        let body = read_to_string(&path).unwrap();
        assert!(body.contains("first run"), "{body}");
        assert!(body.contains("second run"), "{body}");
        assert_eq!(body.lines().count(), 2, "{body}");
    }
}
