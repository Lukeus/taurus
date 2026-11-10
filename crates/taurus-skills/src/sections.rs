//! A long skill, read a section at a time.
//!
//! `load_skill` hands the model a skill's whole procedure, and that text is
//! re-sent on every later step of the turn. For the usual skill — a page or
//! two — that's the point. For a long one, a reference manual that happens to
//! be a skill, most of it is about cases this turn isn't in, and paying for all
//! of it on every step is the cost of having written it down.
//!
//! So past a length, `load_skill` answers with the opening and a list of the
//! headings, and the model asks for the section it needs by name. Below that
//! length nothing changes. The split is on Markdown headings, the one structure
//! every skill already has, and a `#` inside a fenced code block — a shell
//! comment, say — is not a heading.

/// Past this many characters, a skill is offered by section.
///
/// About three thousand tokens. A skill shorter than that costs less in full
/// than the extra round trip a section costs.
pub const LONG_SKILL_CHARS: usize = 12_000;

/// What `section` takes to mean the whole skill.
pub const ALL: &str = "all";

/// One heading and what it covers: its text, up to the next heading at its
/// level or above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// 1 for `#`, 2 for `##`.
    pub level: usize,
    /// The heading's text, without its `#`s.
    pub title: String,
    /// Byte range in the text, heading line included.
    start: usize,
    end: usize,
}

impl Section {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        source[self.start..self.end].trim_end()
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Every heading in `text`, in order, each spanning its subsections.
pub fn sections(text: &str) -> Vec<Section> {
    let mut headings: Vec<(usize, String, usize)> = Vec::new();
    let mut fence: Option<&str> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
        match (fence, marker) {
            (None, Some(m)) => fence = Some(m),
            (Some(open), Some(m)) if open == m => fence = None,
            (None, None) => {
                if let Some((level, title)) = heading(line) {
                    headings.push((level, title, offset));
                }
            }
            _ => {}
        }
        offset += line.len();
    }

    headings
        .iter()
        .enumerate()
        .map(|(i, (level, title, start))| {
            let end = headings[i + 1..]
                .iter()
                .find(|(next, _, _)| next <= level)
                .map(|(_, _, at)| *at)
                .unwrap_or(text.len());
            Section {
                level: *level,
                title: title.clone(),
                start: *start,
                end,
            }
        })
        .collect()
}

/// `## Title` as (2, "Title"). An ATX heading: up to three spaces, one to six
/// `#`s, then a space or the end of the line.
fn heading(line: &str) -> Option<(usize, String)> {
    let line = line.trim_end();
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let level = rest.len() - rest.trim_start_matches('#').len();
    if !(1..=6).contains(&level) {
        return None;
    }
    let after = &rest[level..];
    if !after.is_empty() && !after.starts_with(' ') {
        return None;
    }
    // A closing run of `#`s only counts after a space, so `## Using C#`
    // keeps its `#`.
    let mut title = after.trim();
    let unclosed = title.trim_end_matches('#');
    if unclosed.is_empty() || unclosed.ends_with(' ') {
        title = unclosed;
    }
    let title = title.trim().to_string();
    (!title.is_empty()).then_some((level, title))
}

/// The answer to `load_skill` for a skill `text` long, asked for `section`.
pub fn answer(skill: &str, text: &str, section: Option<&str>) -> Result<String, String> {
    let found = sections(text);
    match section.map(str::trim) {
        Some(asked) if asked.eq_ignore_ascii_case(ALL) => Ok(text.to_string()),
        None if text.len() <= LONG_SKILL_CHARS || found.is_empty() => Ok(text.to_string()),
        None => Ok(outline(skill, text, &found)),
        Some(asked) => pick(skill, text, &found, asked),
    }
}

/// The opening of a long skill, and what else is in it.
fn outline(skill: &str, text: &str, found: &[Section]) -> String {
    // The opening is what comes before the first heading below the title, so
    // a skill that starts `# Name` still shows its introduction.
    let first_body = found
        .iter()
        .find(|section| section.level > 1 || section.start > 0)
        .map(|section| section.start)
        .unwrap_or(text.len());
    let mut out = text[..first_body].trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&format!(
        "[This skill is long ({} characters), so only its opening is above. Its sections:\n",
        text.len()
    ));
    let top = found.iter().map(|s| s.level).min().unwrap_or(1);
    for section in found {
        out.push_str(&format!(
            "{}- {} (~{} characters)\n",
            "  ".repeat(section.level - top),
            section.title,
            section.len()
        ));
    }
    out.push_str(&format!(
        "Call load_skill again with name \"{skill}\" and section set to the heading you need, \
         or to \"{ALL}\" for the whole skill.]"
    ));
    out
}

/// One section, by its heading: the exact heading if there is one, ignoring
/// case, and otherwise the only heading containing what was asked.
fn pick(skill: &str, text: &str, found: &[Section], asked: &str) -> Result<String, String> {
    let lower = asked.trim_start_matches('#').trim().to_lowercase();
    let exact: Vec<&Section> = found
        .iter()
        .filter(|s| s.title.to_lowercase() == lower)
        .collect();
    let candidates = if exact.is_empty() {
        found
            .iter()
            .filter(|s| s.title.to_lowercase().contains(&lower))
            .collect()
    } else {
        exact
    };
    match candidates.as_slice() {
        [one] => Ok(one.text(text).to_string()),
        // The same heading twice is a skill with two "Examples" sections,
        // and the first is as good an answer as either.
        [first, rest @ ..] if rest.iter().all(|s| s.title == first.title) => {
            Ok(first.text(text).to_string())
        }
        [] => Err(format!(
            "The '{skill}' skill has no section called '{asked}'. Its sections: {}",
            titles(found)
        )),
        many => Err(format!(
            "More than one section of '{skill}' matches '{asked}': {}. Ask for one by its \
             whole heading.",
            many.iter()
                .map(|s| format!("\"{}\"", s.title))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn titles(found: &[Section]) -> String {
    if found.is_empty() {
        return "none; it has no headings".into();
    }
    found
        .iter()
        .map(|s| format!("\"{}\"", s.title))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL: &str = "# Deploy\n\nHow to ship.\n\n## Staging\n\nRun it.\n\n```sh\n# not a heading\nmake stage\n```\n\n### Rollback\n\nUndo it.\n\n## Production\n\nCarefully.\n";

    #[test]
    fn headings_span_their_subsections_and_skip_code_fences() {
        let found = sections(SKILL);
        let titles: Vec<&str> = found.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Deploy", "Staging", "Rollback", "Production"]);
        let staging = found[1].text(SKILL);
        assert!(staging.starts_with("## Staging"));
        assert!(staging.contains("# not a heading"));
        assert!(
            staging.contains("Undo it."),
            "a section includes its subsections"
        );
        assert!(!staging.contains("Production"));
    }

    #[test]
    fn a_short_skill_comes_back_whole() {
        assert_eq!(answer("deploy", SKILL, None).unwrap(), SKILL);
    }

    #[test]
    fn a_long_skill_comes_back_as_its_opening_and_an_outline() {
        let long = format!("{SKILL}\n## Appendix\n\n{}", "x".repeat(LONG_SKILL_CHARS));
        let out = answer("deploy", &long, None).unwrap();
        assert!(out.starts_with("# Deploy\n\nHow to ship."), "{out}");
        assert!(!out.contains("Run it."));
        assert!(out.contains("- Staging (~"));
        assert!(
            out.contains("  - Rollback (~"),
            "nested headings are indented"
        );
        assert!(out.contains("section set to the heading"));
    }

    #[test]
    fn a_section_is_found_by_its_heading_in_any_case_or_by_part_of_it() {
        assert!(answer("deploy", SKILL, Some("production"))
            .unwrap()
            .starts_with("## Production"));
        assert!(answer("deploy", SKILL, Some("roll"))
            .unwrap()
            .starts_with("### Rollback"));
        assert!(answer("deploy", SKILL, Some("## Staging"))
            .unwrap()
            .starts_with("## Staging"));
    }

    #[test]
    fn all_is_the_whole_skill() {
        assert_eq!(answer("deploy", SKILL, Some("ALL")).unwrap(), SKILL);
    }

    #[test]
    fn a_section_that_isnt_there_lists_the_ones_that_are() {
        let err = answer("deploy", SKILL, Some("Testing")).unwrap_err();
        assert!(err.contains("\"Staging\""), "{err}");
    }

    #[test]
    fn an_ambiguous_section_names_the_candidates() {
        let err = answer("deploy", SKILL, Some("o")).unwrap_err();
        assert!(err.contains("\"Production\""), "{err}");
        assert!(err.contains("\"Rollback\""), "{err}");
    }

    #[test]
    fn hashes_without_a_space_are_not_a_heading() {
        assert!(heading("#hashtag").is_none());
        assert!(heading("    # indented code").is_none());
        assert_eq!(heading("## Done ##"), Some((2, "Done".into())));
        assert_eq!(heading("## Using C#"), Some((2, "Using C#".into())));
        assert_eq!(heading("## F#"), Some((2, "F#".into())));
    }
}
