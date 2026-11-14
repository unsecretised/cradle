//! The TUI's colours: built-in presets plus `~/.config/cradle/theme.toml`, resolved into
//! ratatui styles.
//!
//! Layers, each on top of the last: the dark preset, the light preset (if chosen), the
//! user's file. Within a layer, palette entries come first, then roles, then elements.
//! An invalid entry is skipped with a warning and the layer below keeps its value. Element
//! colours are resolved last, so changing a role recolours every element that uses it.

use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;

use cradle_config::theme::{StyleSpec, ThemeFile};
use ratatui::style::{Color, Modifier, Style};

/// The dark preset. It lists every element, with comments, so it is also the example
/// printed by `cradle theme --example`.
pub const DARK: &str = include_str!("themes/dark.toml");
/// The light preset, applied on top of the dark one.
const LIGHT: &str = include_str!("themes/light.toml");

const ROLES: &[&str] = &[
    "accent",
    "success",
    "warning",
    "error",
    "muted",
    "on_accent",
];
const MODIFIERS: &[(&str, Modifier)] = &[
    ("bold", Modifier::BOLD),
    ("dim", Modifier::DIM),
    ("italic", Modifier::ITALIC),
    ("underlined", Modifier::UNDERLINED),
    ("reversed", Modifier::REVERSED),
    ("crossed_out", Modifier::CROSSED_OUT),
];
/// The optional whole-screen style.
const BACKGROUND: &str = "ui.background";

macro_rules! theme {
    ($($field:ident = $key:literal,)*) => {
        /// Every style the TUI draws with.
        #[derive(Debug, Clone, PartialEq)]
        pub struct Theme {
            /// The preset the theme starts from.
            pub preset: &'static str,
            /// Whole-screen style. `None` keeps the terminal's own colours.
            pub background: Option<Style>,
            $(pub $field: Style,)*
        }

        /// Every element key (`section.key`) except `ui.background`.
        const ELEMENTS: &[&str] = &[$($key,)*];

        impl Theme {
            fn blank(preset: &'static str) -> Self {
                Theme {
                    preset,
                    background: None,
                    $($field: Style::new(),)*
                }
            }

            fn slot(&mut self, key: &str) -> Option<&mut Style> {
                match key {
                    $($key => Some(&mut self.$field),)*
                    _ => None,
                }
            }
        }
    };
}

theme! {
    brand = "ui.brand",
    mode_read_only = "ui.mode.read_only",
    mode_full = "ui.mode.full",
    input_read_only = "ui.input.read_only",
    input_full = "ui.input.full",
    border = "ui.border",
    border_approval = "ui.border.approval",
    border_help = "ui.border.help",
    selection = "ui.selection",
    cursor = "ui.cursor",
    hint = "ui.hint",
    title = "ui.title",
    note = "ui.note",
    confirm = "ui.confirm",
    prompt = "ui.prompt",
    user_mark = "ui.user.mark",
    user = "ui.user",
    notice = "ui.notice",
    error_mark = "ui.error.mark",
    error = "ui.error",
    thinking = "ui.thinking",
    tool_running = "tool.running",
    tool_ok = "tool.ok",
    tool_failed = "tool.failed",
    tool_detail = "tool.detail",
    diff_added = "diff.added",
    diff_removed = "diff.removed",
    diff_hunk = "diff.hunk",
    diff_header = "diff.header",
    review_safe = "review.safe",
    review_unsafe = "review.unsafe",
    review_unsure = "review.unsure",
    md_h1 = "markdown.h1",
    md_h2 = "markdown.h2",
    md_heading = "markdown.heading",
    md_code = "markdown.code",
    md_code_block = "markdown.code.block",
    md_link = "markdown.link",
    md_image = "markdown.image",
    md_list_marker = "markdown.list.marker",
    md_quote = "markdown.quote",
    md_table_header = "markdown.table.header",
    md_punctuation = "markdown.punctuation",
    setup_selected = "setup.selected",
    setup_provider = "setup.provider",
    setup_id = "setup.id",
    setup_ready = "setup.ready",
    setup_needs_key = "setup.needs_key",
    setup_missing = "setup.missing",
    setup_ok = "setup.ok",
    setup_error = "setup.error",
}

impl Default for Theme {
    /// The dark preset.
    fn default() -> Self {
        Theme::resolve(&ThemeFile::default()).0
    }
}

impl Theme {
    /// Resolve a user theme file on top of its preset. Returns warnings for anything
    /// skipped.
    pub fn resolve(user: &ThemeFile) -> (Theme, Vec<String>) {
        let mut warnings = Vec::new();
        let mut r = Resolver::default();
        // The presets are checked by tests, so their warnings are never shown.
        r.apply(&ThemeFile::parse(DARK).0, &mut Vec::new());
        let preset = match user.preset.as_deref() {
            None | Some("dark") => "dark",
            Some("light") => {
                r.apply(&ThemeFile::parse(LIGHT).0, &mut Vec::new());
                "light"
            }
            Some(other) => {
                warnings.push(format!(
                    "preset: unknown preset `{other}` (expected dark or light), using dark"
                ));
                "dark"
            }
        };
        r.apply(user, &mut warnings);

        let mut theme = Theme::blank(preset);
        for (key, spec) in &r.specs {
            // Every stored spec was checked when applied, and colours only get added.
            let style = r.style(spec).unwrap_or_default();
            if key == BACKGROUND {
                theme.background = (*spec != StyleSpec::default()).then_some(style);
            } else if let Some(slot) = theme.slot(key) {
                *slot = style;
            }
        }
        (theme, warnings)
    }
}

/// Load the user's theme file (`None`: no config directory). Never fails: problems are
/// returned as warnings and the preset fills in.
pub fn load(path: Option<&Path>) -> (Theme, Vec<String>) {
    let Some(path) = path else {
        return (Theme::default(), Vec::new());
    };
    let (file, mut warnings) = ThemeFile::load(path);
    let (theme, more) = Theme::resolve(&file);
    warnings.extend(more);
    (theme, warnings)
}

#[derive(Default)]
struct Resolver {
    palette: BTreeMap<String, Color>,
    roles: BTreeMap<String, Color>,
    specs: BTreeMap<String, StyleSpec>,
}

impl Resolver {
    fn apply(&mut self, file: &ThemeFile, warnings: &mut Vec<String>) {
        for (name, value) in &file.palette {
            match literal(value) {
                Some(c) => {
                    self.palette.insert(name.clone(), c);
                }
                None => warnings.push(format!("palette.{name}: unknown colour `{value}`")),
            }
        }
        for (name, value) in &file.roles {
            if !ROLES.contains(&name.as_str()) {
                warnings.push(format!(
                    "roles.{name}: unknown role (expected one of {})",
                    ROLES.join(", ")
                ));
                continue;
            }
            match self.palette.get(value).copied().or_else(|| literal(value)) {
                Some(c) => {
                    self.roles.insert(name.clone(), c);
                }
                None => warnings.push(format!("roles.{name}: unknown colour `{value}`")),
            }
        }
        for (key, spec) in &file.elements {
            if key != BACKGROUND && !ELEMENTS.contains(&key.as_str()) {
                warnings.push(format!("{key}: unknown element, ignored"));
                continue;
            }
            match self.style(spec) {
                Ok(_) => {
                    self.specs.insert(key.clone(), spec.clone());
                }
                Err(problem) => warnings.push(format!("{key}: {problem}")),
            }
        }
    }

    /// A palette name, then a role, then a colour.
    fn color(&self, name: &str) -> Result<Color, String> {
        self.palette
            .get(name)
            .or_else(|| self.roles.get(name))
            .copied()
            .or_else(|| literal(name))
            .ok_or_else(|| format!("unknown colour `{name}`"))
    }

    fn style(&self, spec: &StyleSpec) -> Result<Style, String> {
        let mut style = Style::new();
        if let Some(fg) = &spec.fg {
            style = style.fg(self.color(fg)?);
        }
        if let Some(bg) = &spec.bg {
            style = style.bg(self.color(bg)?);
        }
        for name in &spec.modifiers {
            let modifier = MODIFIERS
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, m)| *m)
                .ok_or_else(|| {
                    let names: Vec<&str> = MODIFIERS.iter().map(|(n, _)| *n).collect();
                    format!(
                        "unknown modifier `{name}` (expected one of {})",
                        names.join(", ")
                    )
                })?;
            style = style.add_modifier(modifier);
        }
        Ok(style)
    }
}

/// A colour name, `#rrggbb` or a 256-colour index.
fn literal(value: &str) -> Option<Color> {
    Color::from_str(value.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> ThemeFile {
        let (file, warnings) = ThemeFile::parse(src);
        assert!(warnings.is_empty(), "{warnings:?}");
        file
    }

    #[test]
    fn presets_are_complete_and_valid() {
        let dark = parse(DARK);
        for key in ELEMENTS {
            assert!(dark.elements.contains_key(*key), "dark.toml lacks {key}");
        }
        for role in ROLES {
            assert!(
                dark.roles.contains_key(*role),
                "dark.toml lacks role {role}"
            );
        }
        let mut warnings = Vec::new();
        let mut r = Resolver::default();
        r.apply(&dark, &mut warnings);
        r.apply(&parse(LIGHT), &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn dark_matches_the_original_colours() {
        let t = Theme::default();
        assert_eq!(t.preset, "dark");
        assert_eq!(t.background, None);
        assert_eq!(t.hint, Style::new().fg(Color::DarkGray));
        assert_eq!(t.border, Style::new().fg(Color::Cyan));
        assert_eq!(
            t.mode_full,
            Style::new()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD)
        );
        assert_eq!(t.selection, Style::new().fg(Color::Black).bg(Color::Cyan));
        assert_eq!(t.md_code, Style::new().fg(Color::Yellow));
        assert_eq!(
            t.md_h1,
            Style::new()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        );
        assert_eq!(
            t.thinking,
            Style::new()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC)
        );
        assert_eq!(t.border_help, Style::new());
    }

    #[test]
    fn the_example_is_the_dark_preset() {
        let (theme, warnings) = Theme::resolve(&parse(DARK));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(theme, Theme::default());
    }

    #[test]
    fn light_differs_from_dark() {
        let (light, warnings) = Theme::resolve(&parse("preset = \"light\""));
        assert!(warnings.is_empty());
        assert_eq!(light.preset, "light");
        assert_eq!(light.border, Style::new().fg(Color::Blue));
        assert_eq!(light.hint, Style::new().fg(Color::Indexed(244)));
        assert_ne!(light, Theme::default());
    }

    #[test]
    fn roles_cascade_and_palette_names_resolve() {
        let (t, warnings) = Theme::resolve(&parse(
            r##"
            [palette]
            sky = "#61afef"
            [roles]
            accent = "sky"
            [ui]
            cursor = "lightred"
            "##,
        ));
        assert!(warnings.is_empty(), "{warnings:?}");
        let sky = Color::Rgb(0x61, 0xaf, 0xef);
        assert_eq!(
            t.border,
            Style::new().fg(sky),
            "unset elements follow the role"
        );
        assert_eq!(t.md_list_marker, Style::new().fg(sky));
        assert_eq!(t.selection, Style::new().fg(Color::Black).bg(sky));
        assert_eq!(t.cursor, Style::new().fg(Color::LightRed));
    }

    #[test]
    fn element_settings_replace_the_preset_style() {
        let (t, _) = Theme::resolve(&parse("[ui]\n\"mode.full\" = \"red\""));
        assert_eq!(t.mode_full, Style::new().fg(Color::Red));
    }

    #[test]
    fn invalid_values_warn_and_keep_the_preset() {
        let (t, warnings) = Theme::resolve(&parse(
            r#"
            preset = "solarized"
            [palette]
            bad = "notacolour"
            [roles]
            accent = "nope"
            shiny = "red"
            [ui]
            border = "bad"
            hint = { modifiers = ["blink"] }
            nonsense = "red"
            note = "magenta"
            "#,
        ));
        assert_eq!(warnings.len(), 7, "{warnings:?}");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("unknown preset `solarized`"))
        );
        assert!(warnings.iter().any(|w| w.starts_with("ui.nonsense")));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("unknown modifier `blink`"))
        );
        let dark = Theme::default();
        assert_eq!(t.border, dark.border);
        assert_eq!(t.hint, dark.hint);
        assert_eq!(
            t.note,
            Style::new().fg(Color::Magenta),
            "valid entries apply"
        );
    }

    #[test]
    fn background_is_optional() {
        assert_eq!(Theme::default().background, None);
        let (t, _) = Theme::resolve(&parse("[ui]\nbackground = { fg = \"white\", bg = 236 }"));
        assert_eq!(
            t.background,
            Some(Style::new().fg(Color::White).bg(Color::Indexed(236)))
        );
    }

    #[test]
    fn load_reads_the_file_and_tolerates_absence() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("theme.toml");
        assert_eq!(load(Some(&path)), (Theme::default(), Vec::new()));
        assert_eq!(load(None).0, Theme::default());
        std::fs::write(&path, "preset = \"light\"\n[roles]\nerror = 9\nfoo = 1").unwrap();
        let (t, warnings) = load(Some(&path));
        assert_eq!(t.preset, "light");
        assert_eq!(t.error, Style::new().fg(Color::Indexed(9)));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }
}
