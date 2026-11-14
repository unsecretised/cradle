//! Skills: reusable instructions in `.agents/skills/`, either `<name>/SKILL.md` or
//! `<name>.md`. A skill may start with front matter giving its `name` and `description`.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::ConfigError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    /// One line on when to use it ("" if the skill has none).
    pub description: String,
    pub path: PathBuf,
}

impl Skill {
    /// The skill's instructions, without front matter.
    pub fn body(&self) -> std::io::Result<String> {
        let text = fs::read_to_string(&self.path)?;
        Ok(split_front_matter(&text).1.trim().to_owned())
    }
}

/// Split `---`-fenced `key: value` front matter from the rest of a markdown file. Keys are
/// lowercased; values have surrounding quotes removed. Without front matter, the map is
/// empty and the body is the whole text.
pub fn split_front_matter(text: &str) -> (BTreeMap<String, String>, &str) {
    let mut fields = BTreeMap::new();
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (fields, text);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        offset += line.len();
        let line = line.trim();
        if line == "---" {
            for value in fields.values_mut() {
                *value = value.trim_matches(['"', '\'']).to_owned();
            }
            return (fields, &rest[offset..]);
        }
        if let Some((key, value)) = line.split_once(':') {
            fields.insert(key.trim().to_lowercase(), value.trim().to_owned());
        }
    }
    // No closing fence: not front matter.
    (BTreeMap::new(), text)
}

/// The skills in `dir` (`.agents/skills/`), sorted by name.
pub(crate) fn list(dir: &Path) -> Result<Vec<Skill>, ConfigError> {
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ConfigError::io(dir, e)),
    };
    let mut skills = Vec::new();
    for item in read {
        let path = item.map_err(|e| ConfigError::io(dir, e))?.path();
        let (file, default_name) = if path.is_dir() {
            (path.join("SKILL.md"), path.file_name())
        } else if path.extension().is_some_and(|e| e == "md") {
            (path.clone(), path.file_stem())
        } else {
            continue;
        };
        let Some(default_name) = default_name.and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(ConfigError::io(&file, e)),
        };
        let (mut fields, _) = split_front_matter(&text);
        skills.push(Skill {
            name: fields.remove("name").unwrap_or(default_name),
            description: fields.remove("description").unwrap_or_default(),
            path: file,
        });
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_front_matter() {
        let (fields, body) = split_front_matter("---\nName: x\ndescription: \"y: z\"\n---\nbody\n");
        assert_eq!(fields["name"], "x");
        assert_eq!(fields["description"], "y: z");
        assert_eq!(body, "body\n");
        assert_eq!(split_front_matter("plain").1, "plain");
        assert!(split_front_matter("---\nunclosed: yes\n").0.is_empty());
    }

    #[test]
    fn lists_both_layouts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::create_dir_all(dir.join("release")).unwrap();
        fs::write(
            dir.join("release/SKILL.md"),
            "---\ndescription: cut a release\n---\nSteps",
        )
        .unwrap();
        fs::write(dir.join("review.md"), "---\nname: code-review\n---\nLook").unwrap();
        fs::create_dir_all(dir.join("empty")).unwrap();
        fs::write(dir.join("notes.txt"), "ignored").unwrap();

        let skills = list(dir).unwrap();
        let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["code-review", "release"]);
        assert_eq!(skills[1].description, "cut a release");
        assert_eq!(skills[1].body().unwrap(), "Steps");
    }
}
