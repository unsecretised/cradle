//! Conversation transcripts as JSONL under `~/.local/share/cradle/sessions/<project-hash>/`.
//!
//! Each line is a message, or a [`Meta`] line recording the backend in use or a CLI's own
//! session id. Meta lines carry a `meta` key, which messages never have, so transcripts
//! written before meta lines existed still read.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cradle_backends::{Message, Role};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The provider and model a conversation used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedBackend {
    pub provider: String,
    /// The model name, or `default` for a CLI using its own default.
    pub model: String,
}

/// A CLI backend's own session id, which only that provider can resume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliSession {
    pub provider: String,
    pub id: String,
}

/// A transcript line that is not a message. The last one of each kind applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "meta", rename_all = "snake_case")]
pub enum Meta {
    /// The backend used from here on.
    Backend(SavedBackend),
    /// The CLI session the conversation continues in.
    CliSession(CliSession),
}

/// One line of a transcript. `Meta` is tried first: it needs the `meta` key.
#[derive(Deserialize)]
#[serde(untagged)]
enum Entry {
    Meta(Meta),
    Message(Message),
}

/// A transcript as read back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcript {
    pub messages: Vec<Message>,
    /// The backend last recorded.
    pub backend: Option<SavedBackend>,
    /// The CLI session last recorded.
    pub cli_session: Option<CliSession>,
}

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
        self.write_line(message)
    }

    pub fn append_meta(&mut self, meta: &Meta) -> io::Result<()> {
        self.write_line(meta)
    }

    fn write_line(&mut self, value: &impl Serialize) -> io::Result<()> {
        let line = serde_json::to_string(value).map_err(io::Error::other)?;
        writeln!(self.file, "{line}")
    }
}

/// A saved transcript, for choosing one to resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub path: PathBuf,
    /// The file name without `.jsonl`, for `cradle --resume <id>`.
    pub id: String,
    /// Seconds since the Unix epoch when the transcript was started.
    pub started: u64,
    /// The first thing the user said.
    pub title: String,
    pub messages: usize,
}

/// The most recent transcript for `root`.
pub fn latest(data_dir: &Path, root: &Path) -> io::Result<Option<(PathBuf, Transcript)>> {
    let mut files = transcripts(data_dir, root)?;
    let Some(path) = files.pop() else {
        return Ok(None);
    };
    Ok(Some((path.clone(), read(&path)?)))
}

/// The transcript for `root` whose id (file name without `.jsonl`) is `id`, or the only
/// one whose id starts with it.
pub fn find(data_dir: &Path, root: &Path, id: &str) -> io::Result<PathBuf> {
    let files = transcripts(data_dir, root)?;
    if let Some(path) = files.iter().find(|p| file_id(p) == id) {
        return Ok(path.clone());
    }
    let matches: Vec<&PathBuf> = files
        .iter()
        .filter(|p| !id.is_empty() && file_id(p).starts_with(id))
        .collect();
    match matches.as_slice() {
        [path] => Ok((*path).clone()),
        [] => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no conversation `{id}` in this project (see /session)"),
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "`{id}` matches {} conversations; give more of the id",
                matches.len()
            ),
        )),
    }
}

/// Non-empty transcripts for `root`, newest first. Unreadable ones are skipped.
pub fn list(data_dir: &Path, root: &Path) -> io::Result<Vec<SessionInfo>> {
    let mut out = Vec::new();
    for path in transcripts(data_dir, root)?.into_iter().rev() {
        let Ok(transcript) = read(&path) else {
            continue;
        };
        let messages = transcript.messages;
        if messages.is_empty() {
            continue;
        }
        let title = messages
            .iter()
            .find(|m| m.role == Role::User && !m.text().trim().is_empty())
            .map(|m| m.text().trim().lines().next().unwrap_or("").to_owned())
            .unwrap_or_default();
        let id = file_id(&path).to_owned();
        let started = id
            .split('-')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        out.push(SessionInfo {
            path,
            id,
            started,
            title,
            messages: messages.len(),
        });
    }
    Ok(out)
}

/// Transcript files for `root`, oldest first.
fn transcripts(data_dir: &Path, root: &Path) -> io::Result<Vec<PathBuf>> {
    let dir = project_dir(data_dir, root);
    let mut files: Vec<PathBuf> = match fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    files.sort();
    Ok(files)
}

fn file_id(path: &Path) -> &str {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("")
}

/// The messages and meta lines in a transcript.
pub fn read(path: &Path) -> io::Result<Transcript> {
    let mut transcript = Transcript::default();
    for (n, line) in BufReader::new(File::open(path)?).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let entry = serde_json::from_str(&line).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} line {}: {e}", path.display(), n + 1),
            )
        })?;
        match entry {
            Entry::Message(m) => transcript.messages.push(m),
            Entry::Meta(Meta::Backend(b)) => transcript.backend = Some(b),
            Entry::Meta(Meta::CliSession(s)) => transcript.cli_session = Some(s),
        }
    }
    Ok(transcript)
}

fn project_dir(data_dir: &Path, root: &Path) -> PathBuf {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let hash = hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()));
    data_dir.join("sessions").join(&hash[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(provider: &str, model: &str) -> SavedBackend {
        SavedBackend {
            provider: provider.into(),
            model: model.into(),
        }
    }

    #[test]
    fn writes_and_resumes() {
        let data = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        assert!(latest(data.path(), root.path()).unwrap().is_none());

        let mut log = SessionLog::create(data.path(), root.path()).unwrap();
        log.append(&Message::user("hello")).unwrap();
        log.append(&Message::system("mode")).unwrap();

        let (path, transcript) = latest(data.path(), root.path()).unwrap().unwrap();
        assert_eq!(path, log.path());
        assert_eq!(
            transcript.messages,
            vec![Message::user("hello"), Message::system("mode")]
        );
        assert_eq!(transcript.backend, None);
        assert_eq!(transcript.cli_session, None);
    }

    #[test]
    fn meta_lines_round_trip_and_the_last_one_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut log = SessionLog::open(path.clone()).unwrap();
        log.append_meta(&Meta::Backend(backend("claude-code", "default")))
            .unwrap();
        log.append(&Message::user("hi")).unwrap();
        log.append_meta(&Meta::CliSession(CliSession {
            provider: "claude-code".into(),
            id: "s1".into(),
        }))
        .unwrap();
        log.append_meta(&Meta::Backend(backend("codex", "gpt")))
            .unwrap();
        log.append_meta(&Meta::CliSession(CliSession {
            provider: "codex".into(),
            id: "t2".into(),
        }))
        .unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with(r#"{"meta":"backend","provider":"claude-code","model":"default"}"#),
            "{text}"
        );
        let transcript = read(&path).unwrap();
        assert_eq!(transcript.messages, vec![Message::user("hi")]);
        assert_eq!(transcript.backend, Some(backend("codex", "gpt")));
        assert_eq!(
            transcript.cli_session,
            Some(CliSession {
                provider: "codex".into(),
                id: "t2".into()
            })
        );
    }

    #[test]
    fn reads_transcripts_without_meta_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.jsonl");
        fs::write(
            &path,
            "{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}\n\n",
        )
        .unwrap();
        let transcript = read(&path).unwrap();
        assert_eq!(transcript.messages, vec![Message::user("hi")]);
        assert_eq!(transcript.backend, None);

        fs::write(&path, "{\"nonsense\":1}\n").unwrap();
        assert!(read(&path).unwrap_err().to_string().contains("line 1"));
    }

    #[test]
    fn lists_non_empty_sessions_newest_first() {
        let data = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let dir = project_dir(data.path(), root.path());
        fs::create_dir_all(&dir).unwrap();
        let mut old = SessionLog::open(dir.join("000000000001-1.jsonl")).unwrap();
        old.append_meta(&Meta::Backend(backend("a", "m"))).unwrap();
        old.append(&Message::system("mode")).unwrap();
        old.append(&Message::user("first\nmore")).unwrap();
        let mut new = SessionLog::open(dir.join("000000000002-1.jsonl")).unwrap();
        new.append(&Message::user("second")).unwrap();
        SessionLog::open(dir.join("000000000003-1.jsonl")).unwrap();
        let mut meta_only = SessionLog::open(dir.join("000000000004-1.jsonl")).unwrap();
        meta_only
            .append_meta(&Meta::Backend(backend("a", "m")))
            .unwrap();

        let sessions = list(data.path(), root.path()).unwrap();
        let titles: Vec<&str> = sessions.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            ["second", "first"],
            "transcripts without messages are skipped"
        );
        assert_eq!(sessions[1].id, "000000000001-1");
        assert_eq!(sessions[1].started, 1);
        assert_eq!(sessions[1].messages, 2, "meta lines are not messages");
    }

    #[test]
    fn finds_sessions_by_id_or_unique_prefix() {
        let data = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let dir = project_dir(data.path(), root.path());
        fs::create_dir_all(&dir).unwrap();
        for name in ["000000000010-1", "000000000020-1", "000000000020-2"] {
            SessionLog::open(dir.join(format!("{name}.jsonl"))).unwrap();
        }
        let find = |id: &str| find(data.path(), root.path(), id);

        assert_eq!(
            find("000000000020-1").unwrap(),
            dir.join("000000000020-1.jsonl")
        );
        assert_eq!(
            find("0000000000").unwrap_err().kind(),
            io::ErrorKind::InvalidInput,
            "ambiguous"
        );
        assert_eq!(
            find("000000000010").unwrap(),
            dir.join("000000000010-1.jsonl")
        );
        assert_eq!(find("9").unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(find("").unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
