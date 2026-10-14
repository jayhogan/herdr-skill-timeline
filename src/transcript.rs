//! Locating Claude transcripts and reading only what was appended since the
//! last read.

use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// `${CLAUDE_CONFIG_DIR:-~/.claude}/projects`.
pub fn projects_dir() -> PathBuf {
    let base = env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".claude"));
    base.join("projects")
}

/// Finds `<projects_dir>/*/<session_id>.jsonl`. Any project directory may
/// hold it (Claude names them after the session's starting cwd).
pub fn find_transcript(projects_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let file_name = format!("{session_id}.jsonl");
    fs::read_dir(projects_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&file_name))
        .find(|path| path.is_file())
}

/// Incremental reader for one append-only file.
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    offset: u64,
}

impl Tail {
    pub fn new(path: PathBuf) -> Self {
        Self { path, offset: 0 }
    }

    /// Complete lines appended since the last call. A trailing line without
    /// a newline (Claude mid-write) is left for the next call.
    pub fn read_new_lines(&mut self) -> io::Result<Vec<String>> {
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        let Some(last_newline) = buf.iter().rposition(|&b| b == b'\n') else {
            return Ok(vec![]);
        };
        let complete = &buf[..=last_newline];
        self.offset += complete.len() as u64;
        Ok(String::from_utf8_lossy(complete)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn finds_transcript_in_any_project_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("-Users-a-repo")).unwrap();
        fs::create_dir_all(dir.path().join("-Users-a-repo-worktree")).unwrap();
        let target = dir.path().join("-Users-a-repo-worktree").join("abc.jsonl");
        fs::write(&target, "").unwrap();
        assert_eq!(find_transcript(dir.path(), "abc"), Some(target));
        assert_eq!(find_transcript(dir.path(), "missing"), None);
        assert_eq!(find_transcript(&dir.path().join("nope"), "abc"), None);
    }

    #[test]
    fn reads_incrementally_and_holds_partial_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        fs::write(&path, "one\ntwo\nthr").unwrap();
        let mut tail = Tail::new(path.clone());
        assert_eq!(tail.read_new_lines().unwrap(), vec!["one", "two"]);
        assert!(tail.read_new_lines().unwrap().is_empty());

        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(f, "ee\nfour\n").unwrap();
        assert_eq!(tail.read_new_lines().unwrap(), vec!["three", "four"]);
        assert!(tail.read_new_lines().unwrap().is_empty());
    }

    #[test]
    fn missing_file_is_an_error_not_a_panic() {
        let mut tail = Tail::new(PathBuf::from("/definitely/not/here.jsonl"));
        assert!(tail.read_new_lines().is_err());
    }
}
