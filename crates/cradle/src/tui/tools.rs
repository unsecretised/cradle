//! Tool calls in the transcript. Consecutive calls whose kind groups (reads, searches,
//! listings) share one card, e.g. "Read 5 files, searched 2 patterns".

use cradle_tools::{ActivityKind, ToolActivity};

use super::{Item, ToolState};

pub(crate) struct ToolEntry {
    pub id: String,
    pub summary: String,
    pub activity: Option<ToolActivity>,
    pub state: ToolState,
    pub preview: String,
    /// Parts that failed in a call that otherwise worked.
    pub failed: Vec<String>,
}

impl ToolEntry {
    pub fn new(id: String, summary: String, activity: Option<ToolActivity>) -> Self {
        ToolEntry {
            id,
            summary,
            activity,
            state: ToolState::Running,
            preview: String::new(),
            failed: Vec::new(),
        }
    }

    pub fn groups(&self) -> bool {
        self.activity.as_ref().is_some_and(|a| a.kind.groups())
    }

    fn count(&self) -> usize {
        self.activity.as_ref().map_or(1, |a| a.count)
    }
}

/// One card in the transcript: a single call, or a run of groupable calls.
pub(crate) struct ToolGroup {
    pub calls: Vec<ToolEntry>,
}

impl ToolGroup {
    /// A single call that covers one item: shown as its exact action.
    pub fn is_single(&self) -> bool {
        matches!(self.calls.as_slice(), [only] if only.count() == 1 || !only.groups())
    }

    /// The card's title: the exact action for a single call, otherwise counts per kind in
    /// the order they first appeared.
    pub fn title(&self) -> String {
        if self.is_single() {
            return self.calls[0].summary.clone();
        }
        let mut counts: Vec<(ActivityKind, usize)> = Vec::new();
        for activity in self.calls.iter().filter_map(|c| c.activity.as_ref()) {
            match counts.iter_mut().find(|(kind, _)| *kind == activity.kind) {
                Some((_, n)) => *n += activity.count,
                None => counts.push((activity.kind, activity.count)),
            }
        }
        let text = counts
            .iter()
            .map(|(kind, n)| kind.phrase(*n))
            .collect::<Vec<_>>()
            .join(", ");
        let mut chars = text.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect(),
            None => text,
        }
    }

    /// Running while any call runs; failed if anything failed.
    pub fn state(&self) -> ToolState {
        if self.calls.iter().any(|c| c.state == ToolState::Running) {
            ToolState::Running
        } else if self.failed_count() > 0 {
            ToolState::Failed
        } else {
            ToolState::Ok
        }
    }

    /// Items that failed: every item of a failed call, plus failed parts of the others.
    pub fn failed_count(&self) -> usize {
        self.calls
            .iter()
            .map(|c| match c.state {
                ToolState::Failed => c.count(),
                _ => c.failed.len(),
            })
            .sum()
    }
}

/// Add a started call: it joins the last card if both it and that card group.
pub(crate) fn push(items: &mut Vec<Item>, entry: ToolEntry) {
    if entry.groups()
        && let Some(Item::Tools(group)) = items.last_mut()
        && group.calls.iter().all(ToolEntry::groups)
    {
        group.calls.push(entry);
        return;
    }
    items.push(Item::Tools(ToolGroup { calls: vec![entry] }));
}

/// Record a finished call.
pub(crate) fn finish(items: &mut [Item], id: &str, ok: bool, preview: String, failed: Vec<String>) {
    for item in items.iter_mut().rev() {
        if let Item::Tools(group) = item
            && let Some(entry) = group.calls.iter_mut().find(|c| c.id == id)
        {
            entry.state = if ok { ToolState::Ok } else { ToolState::Failed };
            entry.preview = preview;
            entry.failed = failed;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cradle_tools::{FileRange, ToolCall};

    fn entry(id: &str, call: ToolCall) -> ToolEntry {
        ToolEntry::new(id.into(), call.summary(), Some(call.activity()))
    }

    fn read(path: &str) -> ToolCall {
        ToolCall::ReadFile {
            path: path.into(),
            offset: None,
            limit: None,
        }
    }

    fn reads(paths: &[&str]) -> ToolCall {
        ToolCall::ReadFiles {
            files: paths
                .iter()
                .map(|p| FileRange {
                    path: (*p).into(),
                    offset: None,
                    limit: None,
                })
                .collect(),
        }
    }

    fn search(pattern: &str) -> ToolCall {
        ToolCall::Search {
            pattern: pattern.into(),
            path: None,
            glob: None,
        }
    }

    fn group(items: &[Item], i: usize) -> &ToolGroup {
        match &items[i] {
            Item::Tools(g) => g,
            _ => panic!("item {i} is not a tool card"),
        }
    }

    #[test]
    fn reads_searches_and_lists_group_together() {
        let mut items = Vec::new();
        push(&mut items, entry("1", read("a.rs")));
        assert_eq!(
            group(&items, 0).title(),
            "read a.rs",
            "one call: exact action"
        );
        push(&mut items, entry("2", reads(&["b", "c"])));
        push(&mut items, entry("3", search("fn")));
        push(&mut items, entry("4", read("d.rs")));
        push(
            &mut items,
            entry(
                "5",
                ToolCall::ListFiles {
                    path: None,
                    glob: None,
                },
            ),
        );
        assert_eq!(items.len(), 1);
        assert_eq!(
            group(&items, 0).title(),
            "Read 4 files, searched 1 pattern, listed 1 directory"
        );
    }

    #[test]
    fn other_tools_and_items_break_groups() {
        let mut items = Vec::new();
        push(&mut items, entry("1", read("a")));
        push(
            &mut items,
            entry(
                "2",
                ToolCall::EditFile {
                    path: "a".into(),
                    old_string: "x".into(),
                    new_string: "y".into(),
                    replace_all: false,
                },
            ),
        );
        push(&mut items, entry("3", read("b")));
        items.push(Item::Assistant("thinking out loud".into()));
        push(&mut items, entry("4", read("c")));
        push(&mut items, ToolEntry::new("5".into(), "bash".into(), None));
        assert_eq!(items.len(), 6);
        assert_eq!(group(&items, 1).title(), "edit a");
        assert_eq!(group(&items, 5).title(), "bash");
    }

    #[test]
    fn single_read_files_call_shows_counts() {
        let mut items = Vec::new();
        push(&mut items, entry("1", reads(&["a", "b"])));
        assert_eq!(group(&items, 0).title(), "Read 2 files");
    }

    #[test]
    fn failures_are_counted() {
        let mut items = Vec::new();
        push(&mut items, entry("1", reads(&["a", "b"])));
        push(&mut items, entry("2", read("c")));
        push(&mut items, entry("3", read("d")));
        assert_eq!(group(&items, 0).state(), ToolState::Running);
        finish(
            &mut items,
            "1",
            true,
            String::new(),
            vec!["b: missing".into()],
        );
        finish(&mut items, "2", false, "cannot read".into(), vec![]);
        assert_eq!(group(&items, 0).state(), ToolState::Running);
        finish(&mut items, "3", true, String::new(), vec![]);
        let g = group(&items, 0);
        assert_eq!((g.state(), g.failed_count()), (ToolState::Failed, 2));
    }
}
