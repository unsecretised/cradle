//! `/preferences` (alias `/soul`): the user's behaviour preferences, asked as a batch of
//! questions in the question modal. Also opened after the setup screen. Saved to
//! `~/.config/cradle/soul.toml`.

use cradle_config::soul::{Audience, Detail, QUESTIONS_RANGE, Soul};
use cradle_core::Answer;
use cradle_tools::Question;

/// The questions, in order: questions scale, audience, detail, auto_verify.
pub fn questions() -> Vec<Question> {
    let scale = QUESTIONS_RANGE
        .map(|n| match n {
            1 => "1 · only when it's blocked".to_owned(),
            5 => "5 · when a decision is yours to make".to_owned(),
            10 => "10 · before every decision".to_owned(),
            n => n.to_string(),
        })
        .collect();
    vec![
        Question::single("How often should the agent ask you questions?", scale),
        Question::single(
            "How technical should the agent be with you?",
            vec![
                "Technical: precise technical terms".to_owned(),
                "Non-technical: plain language, terms explained".to_owned(),
            ],
        ),
        Question::single(
            "How much should the agent explain?",
            vec![
                "Concise: what changed and why".to_owned(),
                "Detailed: reasoning and trade-offs too".to_owned(),
            ],
        ),
        Question::single(
            "Check build and tests automatically after the agent changes files?",
            vec![
                "Off: the agent runs them when it sees fit".to_owned(),
                "On: Cradle runs `build` and `test` after edits (usual approvals apply)".to_owned(),
            ],
        ),
    ]
}

/// The option to highlight first on each question: the current value.
pub fn defaults(soul: &Soul) -> Vec<usize> {
    vec![
        usize::from(soul.questions.saturating_sub(1)),
        usize::from(soul.audience == Audience::NonTechnical),
        usize::from(soul.detail == Detail::Detailed),
        usize::from(soul.auto_verify),
    ]
}

/// `current` with the answers applied. A typed answer that doesn't fit keeps the current
/// value; the names of those questions are returned.
pub fn apply(current: &Soul, answers: &[Answer]) -> (Soul, Vec<&'static str>) {
    const NAMES: [&str; 4] = ["questions", "audience", "detail", "auto_verify"];
    let qs = questions();
    let mut soul = *current;
    let mut ignored = Vec::new();
    for (i, answer) in answers.iter().enumerate().take(qs.len()) {
        let index = match answer {
            Answer::Choice(c) => qs[i].options.iter().position(|o| o == c),
            Answer::Text(t) => typed(i, t),
            Answer::Choices { .. } => None,
        };
        let Some(index) = index else {
            ignored.push(NAMES[i]);
            continue;
        };
        match i {
            0 => soul.questions = index as u8 + 1,
            1 => {
                soul.audience = if index == 0 {
                    Audience::Technical
                } else {
                    Audience::NonTechnical
                }
            }
            2 => {
                soul.detail = if index == 0 {
                    Detail::Concise
                } else {
                    Detail::Detailed
                }
            }
            _ => soul.auto_verify = index == 1,
        }
    }
    (soul, ignored)
}

/// The option a typed answer means, if it is clear.
fn typed(question: usize, text: &str) -> Option<usize> {
    let t = text.trim().to_lowercase();
    match question {
        0 => t
            .parse::<u8>()
            .ok()
            .filter(|n| QUESTIONS_RANGE.contains(n))
            .map(|n| usize::from(n - 1)),
        1 if t.starts_with("non") || t.contains("plain") => Some(1),
        1 if t.starts_with("tech") => Some(0),
        2 if t.starts_with("detail") => Some(1),
        2 if t.starts_with("concise") || t.starts_with("short") => Some(0),
        3 if matches!(t.as_str(), "on" | "yes" | "true") => Some(1),
        3 if matches!(t.as_str(), "off" | "no" | "false") => Some(0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_point_at_current_values() {
        let qs = questions();
        let soul = Soul {
            questions: 7,
            audience: Audience::NonTechnical,
            detail: Detail::Concise,
            auto_verify: true,
        };
        let d = defaults(&soul);
        assert_eq!(qs[0].options[d[0]], "7");
        assert!(qs[1].options[d[1]].starts_with("Non-technical"));
        assert!(qs[2].options[d[2]].starts_with("Concise"));
        assert!(qs[3].options[d[3]].starts_with("On"));
        assert_eq!(qs[0].options.len(), 10, "fits the 1-9,0 keys");
    }

    #[test]
    fn answers_become_preferences() {
        let qs = questions();
        let answers = [
            Answer::Choice(qs[0].options[1].clone()),
            Answer::Choice(qs[1].options[1].clone()),
            Answer::Choice(qs[2].options[1].clone()),
            Answer::Choice(qs[3].options[1].clone()),
        ];
        let (soul, ignored) = apply(&Soul::default(), &answers);
        assert!(ignored.is_empty());
        assert_eq!(
            soul,
            Soul {
                questions: 2,
                audience: Audience::NonTechnical,
                detail: Detail::Detailed,
                auto_verify: true,
            }
        );
    }

    #[test]
    fn typed_answers_are_read_when_clear() {
        let answers = [
            Answer::Text("8".into()),
            Answer::Text("plain please".into()),
            Answer::Text("whatever".into()),
            Answer::Text("yes".into()),
        ];
        let (soul, ignored) = apply(&Soul::default(), &answers);
        assert_eq!(soul.questions, 8);
        assert_eq!(soul.audience, Audience::NonTechnical);
        assert_eq!(soul.detail, Detail::Concise, "kept");
        assert!(soul.auto_verify);
        assert_eq!(ignored, ["detail"]);

        let (soul, ignored) = apply(&Soul::default(), &[Answer::Text("11".into())]);
        assert_eq!(soul.questions, 5);
        assert_eq!(ignored, ["questions"]);
    }
}
