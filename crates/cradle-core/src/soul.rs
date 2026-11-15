//! Which preferences apply: the user's, with the project's values where the user picked
//! them. A project value that differs from the user's is never applied without asking.
//! See `.agents/roadmap/FEATURE_SOUL.md`.

use cradle_config::soul::{Audience, Detail, Soul, SoulKey, SoulPatch};
use cradle_tools::Question;

/// For one preference the project sets differently: whose value applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoulChoice {
    Project,
    Mine,
}

/// The preferences that apply, and the keys the user still has to decide about. Until
/// they do, their own value applies. `choice` returns the stored decision for a key and
/// the project's value.
pub fn resolve(
    user: &Soul,
    project: &SoulPatch,
    choice: impl Fn(SoulKey, &str) -> Option<SoulChoice>,
) -> (Soul, Vec<SoulKey>) {
    let theirs = user.with(project);
    let mut soul = *user;
    let mut open = Vec::new();
    for key in user.differences(project) {
        match choice(key, &theirs.value(key)) {
            Some(SoulChoice::Project) => soul.take(key, &theirs),
            Some(SoulChoice::Mine) => {}
            None => open.push(key),
        }
    }
    (soul, open)
}

/// What a preference means, for questions and notices.
pub fn title(key: SoulKey) -> &'static str {
    match key {
        SoulKey::Questions => "how often the agent asks you questions",
        SoulKey::Audience => "how technical the agent is",
        SoulKey::Detail => "how much the agent explains",
        SoulKey::AutoVerify => "checking build and tests after edits",
    }
}

/// A value in words.
pub fn describe(key: SoulKey, soul: &Soul) -> String {
    match key {
        SoulKey::Questions => format!("{} of 10", soul.questions),
        SoulKey::Audience => match soul.audience {
            Audience::Technical => "technical terms".into(),
            Audience::NonTechnical => "plain language".into(),
        },
        SoulKey::Detail => match soul.detail {
            Detail::Concise => "concise".into(),
            Detail::Detailed => "detailed".into(),
        },
        SoulKey::AutoVerify => if soul.auto_verify { "on" } else { "off" }.into(),
    }
}

/// The two options for a key, project's first.
pub fn options(key: SoulKey, user: &Soul, project: &SoulPatch) -> [String; 2] {
    let theirs = user.with(project);
    [
        format!("Use the project's: {}", describe(key, &theirs)),
        format!("Keep mine: {}", describe(key, user)),
    ]
}

/// Ask which value applies for `key`.
pub fn question(key: SoulKey, user: &Soul, project: &SoulPatch) -> Question {
    Question::single(
        format!(
            "This project sets a different preference for {}. Which should apply here?",
            title(key)
        ),
        options(key, user, project).to_vec(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_choices_apply_and_the_rest_stay_open() {
        let user = Soul::default();
        let project = SoulPatch {
            questions: Some(9),
            audience: Some(Audience::NonTechnical),
            detail: Some(Detail::Concise), // same as the user's: nothing to ask
            auto_verify: None,
        };
        let (soul, open) = resolve(&user, &project, |_, _| None);
        assert_eq!(soul, user, "nothing applies before the user decides");
        assert_eq!(open, [SoulKey::Questions, SoulKey::Audience]);

        let (soul, open) = resolve(&user, &project, |key, value| match key {
            SoulKey::Questions => {
                assert_eq!(value, "9");
                Some(SoulChoice::Project)
            }
            _ => Some(SoulChoice::Mine),
        });
        assert!(open.is_empty());
        assert_eq!(soul.questions, 9);
        assert_eq!(soul.audience, Audience::Technical);
    }

    #[test]
    fn questions_name_both_values() {
        let project = SoulPatch {
            questions: Some(2),
            ..SoulPatch::default()
        };
        let q = question(SoulKey::Questions, &Soul::default(), &project);
        assert_eq!(
            q.options,
            ["Use the project's: 2 of 10", "Keep mine: 5 of 10"]
        );
    }
}
