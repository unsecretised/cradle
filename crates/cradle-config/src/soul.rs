//! Behaviour preferences ("Preferences" in the UI): how often the agent asks questions,
//! how technical it is, how much it explains, and whether the harness checks builds after
//! edits. See `.agents/roadmap/FEATURE_SOUL.md`.
//!
//! The user's own are in `~/.config/cradle/soul.toml`. A project may set some of them in
//! `.agents/soul.toml` (or `[soul]` in `agents.toml`); those apply only once the user
//! trusts the config and picks them over their own.
//!
//! ```toml
//! questions = 5            # 1-10: 1 = ask only when blocked, 10 = check in on every decision
//! audience = "technical"   # technical | non-technical
//! detail = "concise"       # concise | detailed
//! auto_verify = false      # run build/test after edits
//! ```

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// The questions scale: 1 = ask only when blocked, 10 = check in on every decision.
pub const QUESTIONS_RANGE: std::ops::RangeInclusive<u8> = 1..=10;

/// Who the agent is talking to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Audience {
    #[default]
    Technical,
    NonTechnical,
}

/// How much the agent explains.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Detail {
    #[default]
    Concise,
    Detailed,
}

/// A complete set of preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Soul {
    pub questions: u8,
    pub audience: Audience,
    pub detail: Detail,
    pub auto_verify: bool,
}

impl Default for Soul {
    fn default() -> Self {
        Soul {
            questions: 5,
            audience: Audience::default(),
            detail: Detail::default(),
            auto_verify: false,
        }
    }
}

/// Preferences as written in a file: anything left out keeps the value underneath.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoulPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub questions: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Audience>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Detail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_verify: Option<bool>,
}

/// One preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SoulKey {
    Questions,
    Audience,
    Detail,
    AutoVerify,
}

impl SoulKey {
    pub const ALL: [SoulKey; 4] = [
        SoulKey::Questions,
        SoulKey::Audience,
        SoulKey::Detail,
        SoulKey::AutoVerify,
    ];

    /// The key as written in `soul.toml`.
    pub fn name(self) -> &'static str {
        match self {
            SoulKey::Questions => "questions",
            SoulKey::Audience => "audience",
            SoulKey::Detail => "detail",
            SoulKey::AutoVerify => "auto_verify",
        }
    }
}

impl SoulPatch {
    /// Parse and check a `soul.toml`. `path` is for error messages.
    pub fn parse(src: &str, path: &Path) -> Result<Self, ConfigError> {
        let patch: SoulPatch = toml::from_str(src).map_err(|source| ConfigError::Toml {
            path: path.to_owned(),
            source,
        })?;
        patch.validate(path)?;
        Ok(patch)
    }

    /// Values TOML cannot rule out by itself.
    pub fn validate(&self, path: &Path) -> Result<(), ConfigError> {
        match self.questions {
            Some(q) if !QUESTIONS_RANGE.contains(&q) => Err(ConfigError::Invalid {
                path: path.to_owned(),
                message: format!("questions = {q}: must be from 1 to 10"),
            }),
            _ => Ok(()),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == SoulPatch::default()
    }

    /// The keys this patch sets, with their values as written in `soul.toml`.
    pub fn entries(&self) -> Vec<(SoulKey, String)> {
        let full = Soul::default().with(self);
        SoulKey::ALL
            .into_iter()
            .filter(|key| self.sets(*key))
            .map(|key| (key, full.value(key)))
            .collect()
    }

    pub fn sets(&self, key: SoulKey) -> bool {
        match key {
            SoulKey::Questions => self.questions.is_some(),
            SoulKey::Audience => self.audience.is_some(),
            SoulKey::Detail => self.detail.is_some(),
            SoulKey::AutoVerify => self.auto_verify.is_some(),
        }
    }
}

impl Soul {
    /// Load the user's `soul.toml`. `Ok(None)`: there is none yet.
    pub fn load(path: &Path) -> Result<Option<Self>, ConfigError> {
        match fs::read_to_string(path) {
            Ok(src) => Ok(Some(Soul::default().with(&SoulPatch::parse(&src, path)?))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ConfigError::io(path, e)),
        }
    }

    /// Write every value, so the file shows what applies.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string(self).map_err(io::Error::other)?;
        fs::write(path, text)
    }

    /// These preferences with `patch` applied on top.
    pub fn with(mut self, patch: &SoulPatch) -> Self {
        self.questions = patch.questions.unwrap_or(self.questions);
        self.audience = patch.audience.unwrap_or(self.audience);
        self.detail = patch.detail.unwrap_or(self.detail);
        self.auto_verify = patch.auto_verify.unwrap_or(self.auto_verify);
        self
    }

    /// Copy one value from `other`.
    pub fn take(&mut self, key: SoulKey, other: &Soul) {
        match key {
            SoulKey::Questions => self.questions = other.questions,
            SoulKey::Audience => self.audience = other.audience,
            SoulKey::Detail => self.detail = other.detail,
            SoulKey::AutoVerify => self.auto_verify = other.auto_verify,
        }
    }

    /// A value as written in `soul.toml`.
    pub fn value(&self, key: SoulKey) -> String {
        match key {
            SoulKey::Questions => self.questions.to_string(),
            SoulKey::Audience => match self.audience {
                Audience::Technical => "technical".into(),
                Audience::NonTechnical => "non-technical".into(),
            },
            SoulKey::Detail => match self.detail {
                Detail::Concise => "concise".into(),
                Detail::Detailed => "detailed".into(),
            },
            SoulKey::AutoVerify => self.auto_verify.to_string(),
        }
    }

    /// The keys `patch` sets to something other than these preferences.
    pub fn differences(&self, patch: &SoulPatch) -> Vec<SoulKey> {
        let theirs = self.with(patch);
        SoulKey::ALL
            .into_iter()
            .filter(|key| patch.sets(*key) && theirs.value(*key) != self.value(*key))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_none_and_saved_file_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg/soul.toml");
        assert_eq!(Soul::load(&path).unwrap(), None);

        let soul = Soul {
            questions: 8,
            audience: Audience::NonTechnical,
            detail: Detail::Detailed,
            auto_verify: true,
        };
        soul.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("audience = \"non-technical\""), "{text}");
        assert_eq!(Soul::load(&path).unwrap(), Some(soul));
    }

    #[test]
    fn partial_files_keep_defaults() {
        let p = SoulPatch::parse("questions = 2", Path::new("soul.toml")).unwrap();
        let soul = Soul::default().with(&p);
        assert_eq!(soul.questions, 2);
        assert_eq!(soul.audience, Audience::Technical);
        assert_eq!(soul.detail, Detail::Concise);
        assert!(!soul.auto_verify);
        assert_eq!(p.entries(), [(SoulKey::Questions, "2".to_owned())]);
        assert!(SoulPatch::parse("", Path::new("s")).unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_values() {
        let path = Path::new("soul.toml");
        for bad in ["questions = 0", "questions = 11", "questions = 300"] {
            assert!(
                SoulPatch::parse(bad, path).is_err(),
                "{bad} should be rejected"
            );
        }
        assert!(matches!(
            SoulPatch::parse("audience = \"expert\"", path),
            Err(ConfigError::Toml { .. })
        ));
        assert!(matches!(
            SoulPatch::parse("questons = 3", path),
            Err(ConfigError::Toml { .. })
        ));
    }

    #[test]
    fn differences_list_only_changed_keys() {
        let mine = Soul::default();
        let patch = SoulPatch {
            questions: Some(5),
            audience: Some(Audience::NonTechnical),
            detail: None,
            auto_verify: Some(true),
        };
        assert_eq!(
            mine.differences(&patch),
            [SoulKey::Audience, SoulKey::AutoVerify]
        );
        let mut theirs = mine;
        theirs.take(SoulKey::Audience, &mine.with(&patch));
        assert_eq!(theirs.audience, Audience::NonTechnical);
        assert!(!theirs.auto_verify);
    }
}
