//! Custom themes: the palette, typefaces, wordmark and shape a window paints in.
//!
//! The app ships two palettes, and everything below the top of `styles.css`
//! speaks in *roles* rather than in colours — a panel is `--bg-raised`, a
//! hairline is `--rule`, the lead accent is `--accent`. Which is why a theme
//! here is such a small object: the raw values those roles resolve to are
//! named exactly once per palette, so a theme has to supply fourteen colours
//! and every one of six thousand lines of stylesheet follows it. Anything
//! wider than that would be a fork of the design system rather than a theme.
//!
//! A theme is a file, for the reason every other preference in Taurus is a
//! file: `~/.taurus/themes/midnight.json` can be diffed, committed, pasted
//! into an issue and handed to a colleague, and the editor in Settings is a
//! convenience over that rather than the only way in. Both layers are read,
//! so a repository can carry its own branding in `.taurus/themes/` and have
//! it apply to everybody who opens that folder — trust-gated like every other
//! thing a workspace can contribute, because a theme names a *file path* for
//! its logo and reading one from a repository you have not vouched for is not
//! something to do quietly.
//!
//! What a theme may not do is as deliberate as what it may. There is no
//! stylesheet, no selector, no length that is not one of the three named
//! below: a theme that could restate a rule could break a layout in a way
//! only its author could reproduce, and "the app is broken" would be a
//! support burden nobody could tie back to a colour picker.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config::{scope_dir, Scope};

/// The subdirectory of a config layer that themes live in.
const THEMES_DIR_NAME: &str = "themes";

/// The largest logo that will be inlined, in bytes.
///
/// A logo is base64'd into the resolved theme and travels over IPC on every
/// status refresh, so it is charged to the window's memory and to every
/// refresh rather than fetched once. A quarter of a megabyte is a generous
/// wordmark SVG and a mean photograph, which is the right way round: the
/// former is what this is for.
const MAX_LOGO_BYTES: u64 = 256 * 1024;

/// Every colour a theme may name, and the CSS custom property each one sets.
///
/// The left-hand side is what somebody writes in a theme file, and it is
/// deliberately *not* what the stylesheet calls these. The design system names
/// its accents after the colours they happen to be — `--lk-cyan`, `--lk-peach`,
/// `--lk-mint` — which is fine for a system with one palette and absurd in a
/// theme file, where the whole point is that the accent might be violet. So a
/// theme names the job and this table maps it to the token.
///
/// The order is the order an editor should show them in, grouped surfaces
/// first, and `src/lib/theme.ts` restates that grouping for its fields — with
/// a test that fails if the two lists ever stop agreeing.
pub const COLORS: &[(&str, &str)] = &[
    // The surface ladder: the window, a panel raised off it, a panel raised
    // off that, the hover step between, and the one hairline weight.
    ("ink", "--lk-ink"),
    ("surface-1", "--lk-surface-1"),
    ("surface-2", "--lk-surface-2"),
    ("surface-hover", "--lk-surface-hover"),
    ("line", "--lk-line"),
    // Three weights of text, brightest first.
    ("text", "--lk-text"),
    ("text-dim", "--lk-text-dim"),
    ("text-faint", "--lk-text-faint"),
    // The lead accent, its hover, and what is legible *on* it — a filled
    // button's label is the one colour a theme cannot derive, because it
    // depends on whether the accent came out light or dark.
    ("accent", "--lk-cyan"),
    ("accent-hover", "--lk-cyan-hover"),
    ("on-accent", "--lk-on-cyan"),
    // The three signals. Named for what they mean rather than what they are,
    // for the same reason as the accent.
    ("ok", "--lk-mint"),
    ("warn", "--lk-peach"),
    ("danger", "--lk-red"),
];

/// The three typefaces, and the tokens they set.
pub const FONTS: &[(&str, &str)] = &[
    ("display", "--lk-display"),
    ("body", "--lk-body"),
    ("mono", "--lk-mono-face"),
];

/// A theme as it is written on disk.
///
/// Every field is optional, and that is the feature: the common case is
/// wanting a different accent, and it should cost four lines rather than a
/// transcription of the whole palette. Anything left out falls through to the
/// built-in palette for whichever mode is showing, which also means a theme
/// keeps working when the app adds a token it has never heard of.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ThemeFile {
    /// What the picker calls it. Falls back to the file's own name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Colours for the dark mode, by the names in [`COLORS`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dark: BTreeMap<String, String>,
    /// Colours for the light mode.
    ///
    /// Separate from `dark` rather than a single palette with a `base`,
    /// because "follow the system" is a preference people keep and a theme
    /// that could not honour it would be a theme that quietly takes it away.
    /// A file may fill in one, the other, or both — see [`CustomTheme::modes`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub light: BTreeMap<String, String>,
    /// Typefaces, shared by both modes. A face is a face whatever the ground.
    #[serde(default, skip_serializing_if = "Fonts::is_empty")]
    pub fonts: Fonts,
    #[serde(default, skip_serializing_if = "Brand::is_empty")]
    pub brand: Brand,
    #[serde(default, skip_serializing_if = "Shape::is_empty")]
    pub shape: Shape,
}

/// The three families the stylesheet asks for by name.
///
/// A family, not a stack: the fallbacks after it — `ui-monospace`, `Menlo`,
/// the generics — are the stylesheet's and stay the stylesheet's, so a theme
/// naming a font that is not installed degrades to the same chain everything
/// else falls back through instead of to nothing.
///
/// Whatever is named has to be *installed on the machine*. Tauri's CSP forbids
/// remote stylesheets, so there is no web font to point at and no way for a
/// theme file to bring a typeface with it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Fonts {
    /// Headings and the wordmark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// Everything that is a sentence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Code, paths, counts, and every micro-label in the rail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono: Option<String>,
}

impl Fonts {
    fn is_empty(&self) -> bool {
        self.display.is_none() && self.body.is_none() && self.mono.is_none()
    }
}

/// The two marks in the top-left corner.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Brand {
    /// The word beside the logo. Empty string is a real answer — a mark alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wordmark: Option<String>,
    /// An SVG or PNG, absolute or relative to the theme file.
    ///
    /// A path rather than the image itself, so the file stays something a
    /// person can read, and so a repository's theme can point at a logo that
    /// is already committed beside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
}

impl Brand {
    fn is_empty(&self) -> bool {
        self.wordmark.is_none() && self.logo.is_none()
    }
}

/// How square and how tight the window is.
///
/// Three numbers rather than the whole spacing ladder. The ladder is a
/// *constraint* — `styles.test.ts` fails the build over any distance that
/// steps off it — and handing a theme the ability to redefine it would be
/// handing it the ability to make the app look like nobody measured anything.
/// What is left is the two decisions that read as brand rather than as
/// layout: how round a corner is, and how much air the two columns sit in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Shape {
    /// Multiplier on the corner-radius ladder. 0 is square, 1 is as shipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    /// The centre column's inset, in px. Everything that runs its full width
    /// starts here — topbar, transcript, composer, data pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gutter: Option<u32>,
    /// The rail's text edge, in px.
    #[serde(
        default,
        rename = "rail-gutter",
        skip_serializing_if = "Option::is_none"
    )]
    pub rail_gutter: Option<u32>,
}

impl Shape {
    fn is_empty(&self) -> bool {
        self.radius.is_none() && self.gutter.is_none() && self.rail_gutter.is_none()
    }
}

/// Which of the two modes a theme can actually paint.
///
/// A theme that fills in only one palette is a statement: "this brand is
/// dark". Selecting it therefore has to pin the mode as well, and the mode
/// pills have to say why rather than silently doing nothing — which is what
/// this is read for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ThemeModes {
    /// Both palettes filled in, or neither — a theme that only changes the
    /// typeface and the wordmark is as good in daylight as at night.
    Both,
    DarkOnly,
    LightOnly,
}

/// A theme after it has been read, checked, and had its logo inlined.
///
/// What the frontend gets. The file is what a person edits; this is what the
/// window paints from, and the difference between the two is the parts that
/// need a filesystem — which layer it came from, where it is, and the logo.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CustomTheme {
    /// The file's own name without `.json`, and what settings store.
    pub id: String,
    pub name: String,
    /// Where it is, for the row that offers to open it in an editor.
    pub path: String,
    pub scope: Scope,
    pub dark: BTreeMap<String, String>,
    pub light: BTreeMap<String, String>,
    pub fonts: Fonts,
    pub wordmark: Option<String>,
    /// The logo, read off disk and inlined as a `data:` URI.
    ///
    /// Inlined here rather than served: the webview would need the asset
    /// protocol pointed at an arbitrary path out of a config file, which is a
    /// wider hole than a base64 string is a cost. `None` when the theme names
    /// no logo *or* when the one it names could not be read — the reason for
    /// the second is in `problems`, and a missing logo must not cost the
    /// palette that came with it.
    pub logo: Option<String>,
    pub shape: Shape,
    pub modes: ThemeModes,
}

/// The directory one layer keeps its themes in.
pub fn themes_dir(scope: Scope, workspace: Option<&Path>) -> Option<PathBuf> {
    scope_dir(scope, workspace).map(|d| d.join(THEMES_DIR_NAME))
}

/// Creates a layer's themes directory, and returns it.
///
/// So that "open the themes folder" is a working route on a machine that has
/// never had one, rather than a file manager opening on nothing. The same
/// reasoning as [`crate::config::ensure_mcp_file`].
pub fn ensure_themes_dir(scope: Scope, workspace: Option<&Path>) -> Result<PathBuf, String> {
    let dir = themes_dir(scope, workspace)
        .ok_or_else(|| "no workspace is open, so it has no config directory".to_string())?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Every theme both layers offer, with anything unusable reported.
///
/// Workspace shadows global on a shared id, the same precedence skills and
/// providers already use — a project can override a theme you did not write
/// without editing yours. Trust-gated through [`config_dirs`], so an
/// untrusted folder's themes are not read at all.
///
/// A file that will not parse costs itself and nothing else. The alternative
/// — one bad file emptying the picker — is how a typo in a theme nobody is
/// using takes away the theme somebody is.
pub fn load_themes(workspace: Option<&Path>) -> (Vec<CustomTheme>, Vec<String>) {
    // Gated here rather than per read: an untrusted workspace resolves to no
    // workspace at all, so `themes_dir` gives back `None` for that layer and
    // there is nothing to forget to check further down.
    let workspace = crate::trust::for_reading(workspace);
    let mut found: BTreeMap<String, CustomTheme> = BTreeMap::new();
    let mut problems = Vec::new();

    for scope in [Scope::Global, Scope::Workspace] {
        let Some(dir) = themes_dir(scope, workspace) else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            // Not having a themes directory is the normal state, not a fault.
            continue;
        };
        // Sorted, so the picker's order is the same on every machine and does
        // not follow whatever order the filesystem happens to hand back.
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();

        for path in paths {
            match read_theme(&path, scope) {
                Ok((theme, mut file_problems)) => {
                    problems.append(&mut file_problems);
                    found.insert(theme.id.clone(), theme);
                }
                Err(e) => problems.push(e),
            }
        }
    }

    (found.into_values().collect(), problems)
}

/// One theme by id, from whichever layer offers it.
///
/// Split from [`load_themes`] on purpose, and the split is about what travels.
/// A resolved theme carries its logo inlined as base64, and the *active* one
/// rides on every status the window is pushed — so resolving it must not mean
/// reading and encoding the logo of every other theme on the machine. The full
/// scan is what the picker asks for, once, when somebody opens it.
///
/// Workspace before global, the same precedence [`load_themes`] applies when
/// two layers offer the same id.
pub fn load_theme(workspace: Option<&Path>, id: &str) -> (Option<CustomTheme>, Vec<String>) {
    if id.is_empty() {
        return (None, Vec::new());
    }
    let workspace = crate::trust::for_reading(workspace);
    for scope in [Scope::Workspace, Scope::Global] {
        let Ok(path) = theme_path(scope, workspace, id) else {
            continue;
        };
        if !path.is_file() {
            continue;
        }
        return match read_theme(&path, scope) {
            Ok((theme, problems)) => (Some(theme), problems),
            Err(e) => (None, vec![e]),
        };
    }
    // Named in settings and not on disk — deleted by hand, or a workspace
    // theme in a folder that is no longer trusted. Worth saying: the symptom
    // otherwise is an app that silently stopped wearing the brand.
    (
        None,
        vec![format!(
            "The theme \"{id}\" is set but there is no themes/{id}.json to read."
        )],
    )
}

/// Reads and checks one theme file.
///
/// Returns the theme *and* its non-fatal problems: a colour that is not a
/// colour is dropped and reported, because a theme that is ninety per cent
/// right should paint the ninety per cent. Only a file that cannot be parsed
/// or named at all fails outright.
fn read_theme(path: &Path, scope: Scope) -> Result<(CustomTheme, Vec<String>), String> {
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            format!(
                "{} is not a name a theme can be stored under",
                path.display()
            )
        })?;
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: could not read it — {e}", short(path)))?;
    // Named the same way every other problem here names a file: the folder and
    // the file, not the forty characters of home directory in front of them.
    let file: ThemeFile = serde_json::from_str(&text)
        .map_err(|e| format!("{}: not valid theme JSON — {e}", short(path)))?;

    let mut problems = Vec::new();
    let dark = clean(&file.dark, path, "dark", &mut problems);
    let light = clean(&file.light, path, "light", &mut problems);

    let (logo, logo_problem) = match file.brand.logo.as_deref() {
        None => (None, None),
        Some(logo) => match inline_logo(logo, path) {
            Ok(uri) => (Some(uri), None),
            Err(e) => (None, Some(e)),
        },
    };
    problems.extend(logo_problem);

    let modes = match (dark.is_empty(), light.is_empty()) {
        (true, false) => ThemeModes::LightOnly,
        (false, true) => ThemeModes::DarkOnly,
        // Both filled in, or a theme that recolours nothing and only changes
        // the typeface, the wordmark or the shape.
        _ => ThemeModes::Both,
    };

    let name = if file.name.trim().is_empty() {
        id.clone()
    } else {
        file.name.trim().to_owned()
    };

    Ok((
        CustomTheme {
            id,
            name,
            path: path.display().to_string(),
            scope,
            dark,
            light,
            fonts: file.fonts,
            wordmark: file.brand.wordmark,
            logo,
            shape: clamp_shape(file.shape, path, &mut problems),
            modes,
        },
        problems,
    ))
}

/// Drops anything that is not a colour this app has a token for, and says so.
fn clean(
    palette: &BTreeMap<String, String>,
    path: &Path,
    mode: &str,
    problems: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (key, value) in palette {
        if !COLORS.iter().any(|(name, _)| name == key) {
            problems.push(format!(
                "{}: \"{key}\" in {mode} is not a colour this app has. The names it knows are {}.",
                short(path),
                COLORS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            continue;
        }
        if !is_hex(value) {
            problems.push(format!(
                "{}: {mode}.{key} is \"{value}\", which is not a hex colour. Write it as #rgb, #rrggbb or #rrggbbaa.",
                short(path)
            ));
            continue;
        }
        out.insert(key.clone(), value.clone());
    }
    out
}

/// Keeps the three shape numbers inside what the layout survives.
///
/// Clamped and reported rather than rejected: somebody who typed 400 for a
/// gutter wanted a wider one, and the app answering with the widest it has is
/// a better outcome than answering with the default and no explanation.
fn clamp_shape(mut shape: Shape, path: &Path, problems: &mut Vec<String>) -> Shape {
    // A radius multiplier past 3 turns a 14px corner into a 42px one, which
    // on a 28px-tall row is not a rounded rectangle but a pill.
    if let Some(r) = shape.radius {
        let clamped = r.clamp(0.0, 3.0);
        if (clamped - r).abs() > f32::EPSILON {
            problems.push(format!(
                "{}: shape.radius {r} is outside 0–3, so {clamped} was used.",
                short(path)
            ));
            shape.radius = Some(clamped);
        }
    }
    // Past 96px the centre column loses more width than the rail has.
    for (value, name) in [
        (&mut shape.gutter, "shape.gutter"),
        (&mut shape.rail_gutter, "shape.rail-gutter"),
    ] {
        if let Some(px) = *value {
            if px > 96 {
                problems.push(format!(
                    "{}: {name} {px} is past the 96px maximum, so 96 was used.",
                    short(path)
                ));
                *value = Some(96);
            }
        }
    }
    shape
}

/// Reads a logo and returns it as a `data:` URI.
fn inline_logo(logo: &str, theme_path: &Path) -> Result<String, String> {
    use base64::Engine;

    let path = Path::new(logo);
    // Relative to the theme file rather than to the process's working
    // directory, so a repository can commit `.taurus/themes/logo.svg` beside
    // the theme that names it and have the pair travel together.
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        theme_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    };

    let mime = match resolved
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        // Named rather than sniffed: the window's CSP lists the types it will
        // render, and guessing at a fifth would produce a broken <img> with
        // nothing to say about why.
        _ => {
            return Err(format!(
                "{}: a logo has to be .svg, .png, .jpg or .webp.",
                short(&resolved)
            ))
        }
    };

    let size = std::fs::metadata(&resolved)
        .map_err(|e| format!("{}: could not read the logo — {e}", short(&resolved)))?
        .len();
    if size > MAX_LOGO_BYTES {
        return Err(format!(
            "{}: the logo is {}KB, over the {}KB limit. It travels with the theme on every refresh.",
            short(&resolved),
            size / 1024,
            MAX_LOGO_BYTES / 1024
        ));
    }

    let bytes = std::fs::read(&resolved)
        .map_err(|e| format!("{}: could not read the logo — {e}", short(&resolved)))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

/// Whether `value` is a hex colour CSS will accept.
fn is_hex(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    matches!(digits.len(), 3 | 4 | 6 | 8) && digits.chars().all(|c| c.is_ascii_hexdigit())
}

/// A path as a problem message should name it — the file, and the folder it is
/// in, and not the forty characters of home directory before them.
fn short(path: &Path) -> String {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("theme");
    match path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    {
        Some(dir) => format!("{dir}/{name}"),
        None => name.to_owned(),
    }
}

/// Turns a name into the id its file is stored under.
///
/// Lowercase, and everything that is not a letter or a digit becomes a hyphen.
/// This is also the security boundary on the id: it is the only thing that
/// reaches the filesystem from a caller, and a `..` or a `/` in it would make
/// "save this theme" a way to write anywhere on disk.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        "theme".to_owned()
    } else {
        trimmed
    }
}

/// Where a theme with this id lives in a layer, if the id is one this app
/// would have written.
///
/// The re-slug is the check: an id that does not survive [`slug`] unchanged is
/// not one of ours, and is refused rather than sanitised. Sanitising would
/// mean a caller asking to delete `../../providers` gets a cheerful success
/// about having deleted something else.
fn theme_path(scope: Scope, workspace: Option<&Path>, id: &str) -> Result<PathBuf, String> {
    if id != slug(id) {
        return Err(format!("\"{id}\" is not a theme name this app can store."));
    }
    let dir = themes_dir(scope, workspace)
        .ok_or_else(|| "no workspace is open, so it has no config directory".to_string())?;
    Ok(dir.join(format!("{id}.json")))
}

/// Writes a theme into a layer, and returns where it went.
///
/// Refuses rather than repairs when a colour is not a colour: this is the path
/// the editor takes, and an editor that silently drops the field you just
/// typed is worse than one that says the field is wrong. Loading is the
/// forgiving direction — see [`read_theme`] — because there the file already
/// exists and the alternative is showing nothing.
pub fn save_theme(
    scope: Scope,
    workspace: Option<&Path>,
    id: &str,
    file: &ThemeFile,
) -> Result<PathBuf, String> {
    let path = theme_path(scope, workspace, id)?;
    let problems = validate(file);
    if !problems.is_empty() {
        return Err(problems.join(" "));
    }
    let json = serde_json::to_string_pretty(file)
        .map_err(|e| format!("could not write the theme: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    // A trailing newline, because these are files people open in editors.
    std::fs::write(&path, format!("{json}\n"))
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(path)
}

/// Removes a theme file.
pub fn delete_theme(scope: Scope, workspace: Option<&Path>, id: &str) -> Result<(), String> {
    let path = theme_path(scope, workspace, id)?;
    std::fs::remove_file(&path).map_err(|e| format!("could not delete {}: {e}", path.display()))
}

/// What is wrong with a theme somebody is trying to save, in words that name
/// the field and what to put in it.
pub fn validate(file: &ThemeFile) -> Vec<String> {
    let mut problems = Vec::new();
    for (mode, palette) in [("dark", &file.dark), ("light", &file.light)] {
        for (key, value) in palette {
            if !COLORS.iter().any(|(name, _)| name == key) {
                problems.push(format!("\"{key}\" is not a colour this app has."));
            } else if !is_hex(value) {
                problems.push(format!(
                    "{mode}.{key} is \"{value}\", which is not a hex colour — write it as #rgb, #rrggbb or #rrggbbaa."
                ));
            }
        }
    }
    if let Some(r) = file.shape.radius {
        if !(0.0..=3.0).contains(&r) {
            problems.push(format!("shape.radius has to be between 0 and 3, not {r}."));
        }
    }
    for (value, name) in [
        (file.shape.gutter, "shape.gutter"),
        (file.shape.rail_gutter, "shape.rail-gutter"),
    ] {
        if value.is_some_and(|px| px > 96) {
            problems.push(format!("{name} has to be 96 or less."));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::isolated_home;

    fn write(dir: &Path, name: &str, json: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), json).unwrap();
    }

    #[test]
    fn reads_a_theme_and_keeps_only_the_colours_it_has_tokens_for() {
        let home = isolated_home();
        write(
            &home.path().join("themes"),
            "midnight.json",
            r##"{"name":"Midnight","dark":{"accent":"#b48cff","nonsense":"#fff"}}"##,
        );
        let (themes, problems) = load_themes(None);
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "Midnight");
        assert_eq!(themes[0].dark.get("accent").unwrap(), "#b48cff");
        // Dropped, and said so — a theme that is mostly right paints the part
        // that is, and the part that is not is not a silent no-op.
        assert!(!themes[0].dark.contains_key("nonsense"));
        assert!(problems.iter().any(|p| p.contains("nonsense")));
    }

    #[test]
    fn a_colour_that_is_not_a_colour_is_dropped_with_its_reason() {
        let home = isolated_home();
        write(
            &home.path().join("themes"),
            "broken.json",
            r##"{"dark":{"accent":"rebeccapurple"}}"##,
        );
        let (themes, problems) = load_themes(None);
        assert!(themes[0].dark.is_empty());
        assert!(problems.iter().any(|p| p.contains("not a hex colour")));
    }

    #[test]
    fn falls_back_to_the_file_name_when_the_theme_does_not_name_itself() {
        let home = isolated_home();
        write(
            &home.path().join("themes"),
            "brand-x.json",
            r##"{"dark":{}}"##,
        );
        let (themes, _) = load_themes(None);
        assert_eq!(themes[0].id, "brand-x");
        assert_eq!(themes[0].name, "brand-x");
    }

    #[test]
    fn one_unparseable_file_does_not_cost_the_others() {
        // The whole reason problems are collected rather than returned: a typo
        // in a theme nobody is using must not empty the picker for the one
        // somebody is.
        let home = isolated_home();
        let dir = home.path().join("themes");
        write(
            &dir,
            "good.json",
            r##"{"name":"Good","dark":{"ink":"#000"}}"##,
        );
        write(&dir, "bad.json", "{ this is not json");
        let (themes, problems) = load_themes(None);
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "Good");
        assert!(problems.iter().any(|p| p.contains("bad.json")));
    }

    #[test]
    fn says_which_modes_a_theme_can_paint() {
        let home = isolated_home();
        let dir = home.path().join("themes");
        write(&dir, "night.json", r##"{"dark":{"ink":"#000"}}"##);
        write(&dir, "day.json", r##"{"light":{"ink":"#fff"}}"##);
        write(
            &dir,
            "both.json",
            r##"{"dark":{"ink":"#000"},"light":{"ink":"#fff"}}"##,
        );
        // A theme that recolours nothing is as good in daylight as at night.
        write(&dir, "typeface.json", r##"{"fonts":{"body":"Inter"}}"##);
        let (themes, _) = load_themes(None);
        let mode = |id: &str| themes.iter().find(|t| t.id == id).unwrap().modes;
        assert_eq!(mode("night"), ThemeModes::DarkOnly);
        assert_eq!(mode("day"), ThemeModes::LightOnly);
        assert_eq!(mode("both"), ThemeModes::Both);
        assert_eq!(mode("typeface"), ThemeModes::Both);
    }

    #[test]
    fn refuses_an_id_that_would_write_outside_the_themes_directory() {
        // The one place a caller's string reaches the filesystem.
        let _home = isolated_home();
        let file = ThemeFile::default();
        for id in ["../../providers", "a/b", "..", "Not A Slug"] {
            assert!(
                save_theme(Scope::Global, None, id, &file).is_err(),
                "{id} was accepted"
            );
        }
    }

    #[test]
    fn saving_refuses_a_colour_the_editor_should_have_caught() {
        let _home = isolated_home();
        let mut file = ThemeFile::default();
        file.dark.insert("accent".into(), "not-a-colour".into());
        let e = save_theme(Scope::Global, None, "x", &file).unwrap_err();
        assert!(e.contains("not a hex colour"), "{e}");
    }

    #[test]
    fn round_trips_a_saved_theme() {
        let _home = isolated_home();
        let mut file = ThemeFile {
            name: "Round Trip".into(),
            ..Default::default()
        };
        file.dark.insert("accent".into(), "#b48cff".into());
        file.fonts.body = Some("Inter".into());
        file.shape.radius = Some(0.0);
        save_theme(Scope::Global, None, "round-trip", &file).unwrap();

        let (themes, problems) = load_themes(None);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(themes[0].name, "Round Trip");
        assert_eq!(themes[0].fonts.body.as_deref(), Some("Inter"));
        assert_eq!(themes[0].shape.radius, Some(0.0));
    }

    #[test]
    fn only_writes_the_keys_that_were_set() {
        // The file is a thing people read and diff. A theme that changes one
        // colour should be four lines, not a transcription of the palette.
        let _home = isolated_home();
        let mut file = ThemeFile::default();
        file.dark.insert("accent".into(), "#b48cff".into());
        let path = save_theme(Scope::Global, None, "small", &file).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.contains("light"), "{text}");
        assert!(!text.contains("fonts"), "{text}");
        assert!(!text.contains("shape"), "{text}");
    }

    #[test]
    fn clamps_a_shape_that_would_break_the_layout_and_says_so() {
        let home = isolated_home();
        write(
            &home.path().join("themes"),
            "huge.json",
            r##"{"shape":{"radius":40,"gutter":400}}"##,
        );
        let (themes, problems) = load_themes(None);
        assert_eq!(themes[0].shape.radius, Some(3.0));
        assert_eq!(themes[0].shape.gutter, Some(96));
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn inlines_a_logo_that_sits_beside_its_theme() {
        let home = isolated_home();
        let dir = home.path().join("themes");
        write(&dir, "brand.json", r##"{"brand":{"logo":"mark.svg"}}"##);
        std::fs::write(dir.join("mark.svg"), "<svg/>").unwrap();
        let (themes, problems) = load_themes(None);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(themes[0]
            .logo
            .as_ref()
            .unwrap()
            .starts_with("data:image/svg+xml;base64,"));
    }

    #[test]
    fn a_logo_that_is_not_there_costs_the_logo_and_not_the_palette() {
        let home = isolated_home();
        write(
            &home.path().join("themes"),
            "brand.json",
            r##"{"dark":{"accent":"#b48cff"},"brand":{"logo":"missing.svg"}}"##,
        );
        let (themes, problems) = load_themes(None);
        assert!(themes[0].logo.is_none());
        assert_eq!(themes[0].dark.get("accent").unwrap(), "#b48cff");
        assert!(problems.iter().any(|p| p.contains("logo")));
    }

    #[test]
    fn resolves_one_theme_without_reading_the_rest() {
        let home = isolated_home();
        let dir = home.path().join("themes");
        write(
            &dir,
            "wanted.json",
            r##"{"name":"Wanted","dark":{"ink":"#000"}}"##,
        );
        // A second theme whose logo does not exist. Resolving `wanted` must
        // not report it, because resolving `wanted` must not read it.
        write(&dir, "other.json", r##"{"brand":{"logo":"nope.svg"}}"##);
        let (theme, problems) = load_theme(None, "wanted");
        assert_eq!(theme.unwrap().name, "Wanted");
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn says_so_when_the_theme_in_force_is_not_on_disk() {
        // Deleted by hand, or living in a workspace that is no longer trusted.
        // The symptom otherwise is an app that quietly stopped wearing a brand.
        let _home = isolated_home();
        let (theme, problems) = load_theme(None, "gone");
        assert!(theme.is_none());
        assert!(problems[0].contains("themes/gone.json"), "{problems:?}");
    }

    #[test]
    fn no_theme_set_is_not_a_problem() {
        let _home = isolated_home();
        let (theme, problems) = load_theme(None, "");
        assert!(theme.is_none());
        assert!(problems.is_empty());
    }

    #[test]
    fn slugs_a_name_into_something_a_filesystem_can_hold() {
        assert_eq!(slug("Midnight Violet"), "midnight-violet");
        assert_eq!(slug("  Acme  Corp!!  "), "acme-corp");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("!!!"), "theme");
    }
}
