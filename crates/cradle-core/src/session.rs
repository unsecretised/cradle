//! Conversation transcripts as JSONL, one message per line, under
//! `~/.local/share/cradle/sessions/<project-hash>/`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cradle_backends::Message;
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct SessionLog {
    path: PathBuf,
    file: File,
}

impl SessionLog {
    /// Start a new transcript for `root`.
    pub fn create(data_dir: &Path, root: &Path) -> io::Result<Self> {
        let dir = project_dir(data_dir, root);
        fs::create_dir_all(&dir)?;
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = dir.join(format!("{secs:012}-{}.jsonl", std::process::id()));
        Self::open(path)
    }

    /// Continue writing to an existing transcript.
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(SessionLog { path, file })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&mut self, message: &Message) -> io::Result<()> {
        let line = serde_json::to_string(message).map_err(io::Error::other)?;
        writeln!(self.file, "{line}")
    }
}

/// The most recent transcript for `root`, with its messages.
pub fn latest(data_dir: &Path, root: &Path) -> io::Result<Option<(PathBuf, Vec<Message>)>> {
    let dir = project_dir(data_dir, root);
    let mut files: Vec<PathBuf> = match fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    files.sort();
    let Some(path) = files.pop() else {
        return Ok(None);
    };
    let mut messages = Vec::new();
    for line in BufReader::new(File::open(&path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        messages.push(serde_json::from_str(&line).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            )
        })?);
    }
    Ok(Some((path, messages)))
}

fn project_dir(data_dir: &Path, root: &Path) -> PathBuf {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let hash = hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()));
    data_dir.join("sessions").join(&hash[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_resumes() {
        let data = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        assert!(latest(data.path(), root.path()).unwrap().is_none());

        let mut log = SessionLog::create(data.path(), root.path()).unwrap();
        log.append(&Message::user("hello")).unwrap();
        log.append(&Message::system("mode")).unwrap();

        let (path, messages) = latest(data.path(), root.path()).unwrap().unwrap();
        assert_eq!(path, log.path());
        assert_eq!(
            messages,
            vec![Message::user("hello"), Message::system("mode")]
        );
    }
}
