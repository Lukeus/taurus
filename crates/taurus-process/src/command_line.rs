//! What a command line runs.
//!
//! A standing grant and a hook are both keyed by a program's name, and a line
//! the model writes rarely runs only one: `cd crates/x && cargo test 2>&1 |
//! tail -20` runs three. This takes a line apart where the shell would — at
//! the operators that start another command, and nowhere inside quotes — and
//! names every program it runs, or says why no list of names can say what it
//! does.
//!
//! Conservative wherever it would have to guess. A line that substitutes a
//! command into itself, writes into a file, feeds a program a script from the
//! line, or hands itself to a program whose whole job is to run the rest is
//! reported as beyond a grant, and the names it does find are offered as what
//! they are: the programs the line mentions, not a promise that nothing else
//! runs. A hook can use the names to fire; a grant may only use them when
//! [`Programs::beyond`] is `None`.
//!
//! Below both `taurus-tools` and `taurus-hooks`, so the gate and the hooks
//! agree about what `git status; rm -rf ~` runs.

/// The programs a command line runs, by name, in the order they appear.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Programs {
    /// Each program once, as the line spells it: `./venv/bin/python` stays a
    /// path, so a grant for it does not cover whatever `python` is on PATH.
    pub names: Vec<String>,
    /// Why no list of names covers this line, when none does. Finishes the
    /// sentence "this one …", for the places that say so.
    pub beyond: Option<&'static str>,
}

const SUBSTITUTES: &str = "runs another command inside it";
const REDIRECTS: &str = "writes its output into a file";
const HEREDOC: &str = "feeds a program a script written into the line";
const HANDS_OVER: &str = "hands the rest of the line to another program";
const INLINE_CODE: &str = "runs code written into the line";
const VARIABLE: &str = "names its program through a variable";
const NOTHING: &str = "names no program";

/// Programs whose whole job is to run the rest of their command line.
///
/// A standing grant for one of these is a grant for anything at all, so a
/// command that starts with one is asked about every time. Matched by name, so
/// `/usr/bin/env` is `env` and `pwsh.exe` is `pwsh`.
const WRAPPERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "fish",
    "env",
    "xargs",
    "sudo",
    "doas",
    "su",
    "nohup",
    "time",
    "exec",
    "eval",
    "command",
    "builtin",
    "nice",
    "timeout",
    "stdbuf",
    "watch",
    "cmd",
    "powershell",
    "pwsh",
];

/// Proxies that run their argument line as the command, unchanged.
///
/// Unlike a wrapper, the program after one of these is the program the user
/// would recognize: `rtk cargo test` is a `cargo` command that happens to have
/// its output condensed. So the grant is keyed by what comes after. `rtk
/// proxy <cmd>` is the same proxy declining to condense.
const PASS_THROUGH: &[(&str, &[&str])] = &[("rtk", &["proxy"])];

/// Interpreters, and the flags that take code from the line instead of a file.
///
/// `python3 -c 'anything'` is `sh -c` with a different syntax. A short flag is
/// matched inside a cluster too, so `-uc` counts.
const INLINE_FLAGS: &[(&str, &[char], &[&str])] = &[
    ("python", &['c'], &[]),
    ("python2", &['c'], &[]),
    ("python3", &['c'], &[]),
    ("node", &['e', 'p'], &["--eval", "--print"]),
    ("nodejs", &['e', 'p'], &["--eval", "--print"]),
    ("deno", &[], &["eval"]),
    ("ruby", &['e'], &[]),
    ("perl", &['e', 'E'], &[]),
    ("php", &['r'], &[]),
];

/// `find` actions that run a program on what was found.
const FIND_RUNS: &[&str] = &["-exec", "-execdir", "-ok", "-okdir"];

impl Programs {
    /// Takes `command` apart and names what it runs.
    pub fn of(command: &str) -> Self {
        let chars: Vec<char> = command.chars().collect();
        let mut segments: Vec<Vec<String>> = Vec::new();
        let mut words: Vec<String> = Vec::new();
        let mut word = String::new();
        let mut in_word = false;
        let mut beyond: Option<&'static str> = None;
        let (mut single, mut double) = (false, false);

        fn end_word(words: &mut Vec<String>, word: &mut String, in_word: &mut bool) {
            if *in_word {
                words.push(std::mem::take(word));
                *in_word = false;
            }
        }
        fn end_segment(
            segments: &mut Vec<Vec<String>>,
            words: &mut Vec<String>,
            word: &mut String,
            in_word: &mut bool,
        ) {
            end_word(words, word, in_word);
            if !words.is_empty() {
                segments.push(std::mem::take(words));
            }
        }

        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if single {
                if c == '\'' {
                    single = false;
                } else {
                    word.push(c);
                }
                i += 1;
                continue;
            }
            if double {
                match c {
                    '"' => double = false,
                    '\\' if matches!(next, Some('"' | '\\' | '$' | '`')) => {
                        word.push(next.unwrap_or_default());
                        i += 1;
                    }
                    // Substitution happens inside double quotes too.
                    '$' if next == Some('(') => {
                        beyond.get_or_insert(SUBSTITUTES);
                        end_segment(&mut segments, &mut words, &mut word, &mut in_word);
                        i += 1;
                    }
                    '`' => {
                        beyond.get_or_insert(SUBSTITUTES);
                        end_segment(&mut segments, &mut words, &mut word, &mut in_word);
                    }
                    _ => word.push(c),
                }
                i += 1;
                continue;
            }
            match c {
                '\'' => {
                    single = true;
                    in_word = true;
                }
                '"' => {
                    double = true;
                    in_word = true;
                }
                '\\' => {
                    // A backslash-newline continues the line; anything else
                    // escaped is literal.
                    match next {
                        Some('\n') => {}
                        Some(n) => {
                            word.push(n);
                            in_word = true;
                        }
                        None => {}
                    }
                    i += 1;
                }
                '$' if next == Some('(') => {
                    beyond.get_or_insert(SUBSTITUTES);
                    end_segment(&mut segments, &mut words, &mut word, &mut in_word);
                    i += 1;
                }
                '`' => {
                    beyond.get_or_insert(SUBSTITUTES);
                    end_segment(&mut segments, &mut words, &mut word, &mut in_word);
                }
                '<' | '>' if next == Some('(') => {
                    beyond.get_or_insert(SUBSTITUTES);
                    end_segment(&mut segments, &mut words, &mut word, &mut in_word);
                    i += 1;
                }
                '<' if next == Some('<') => {
                    beyond.get_or_insert(HEREDOC);
                    end_word(&mut words, &mut word, &mut in_word);
                    while chars.get(i) == Some(&'<') {
                        i += 1;
                    }
                    continue;
                }
                '&' if next == Some('>') => {
                    // `&>file` and `&>>file`: both streams into a file, unless
                    // the file is the bit bucket.
                    i += 2;
                    if chars.get(i) == Some(&'>') {
                        i += 1;
                    }
                    end_word(&mut words, &mut word, &mut in_word);
                    i = redirect_target(&chars, i, &mut beyond);
                    continue;
                }
                '>' => {
                    // The fd digits before it (`2>`) are not a word.
                    if in_word && !word.is_empty() && word.chars().all(|d| d.is_ascii_digit()) {
                        word.clear();
                        in_word = false;
                    }
                    end_word(&mut words, &mut word, &mut in_word);
                    i += 1;
                    if chars.get(i) == Some(&'>') {
                        i += 1;
                    }
                    if chars.get(i) == Some(&'&') {
                        // `>&2`, `>&-`: a stream merge reaches nothing new.
                        // `>&file` is a redirect by another spelling.
                        let target = chars.get(i + 1).copied();
                        if matches!(target, Some(t) if t.is_ascii_digit() || t == '-') {
                            i += 1;
                            while matches!(chars.get(i), Some(t) if t.is_ascii_digit() || *t == '-')
                            {
                                i += 1;
                            }
                            continue;
                        }
                        i += 1;
                    }
                    i = redirect_target(&chars, i, &mut beyond);
                    continue;
                }
                '|' => {
                    end_segment(&mut segments, &mut words, &mut word, &mut in_word);
                    if matches!(next, Some('|' | '&')) {
                        i += 1;
                    }
                }
                ';' => {
                    end_segment(&mut segments, &mut words, &mut word, &mut in_word);
                    if next == Some(';') {
                        i += 1;
                    }
                }
                '&' => {
                    end_segment(&mut segments, &mut words, &mut word, &mut in_word);
                    if next == Some('&') {
                        i += 1;
                    }
                }
                '\n' | '\r' | '(' | ')' => {
                    end_segment(&mut segments, &mut words, &mut word, &mut in_word);
                }
                '{' | '}' => end_word(&mut words, &mut word, &mut in_word),
                c if c.is_whitespace() => end_word(&mut words, &mut word, &mut in_word),
                _ => {
                    word.push(c);
                    in_word = true;
                }
            }
            i += 1;
        }
        end_segment(&mut segments, &mut words, &mut word, &mut in_word);

        let mut names: Vec<String> = Vec::new();
        for segment in &segments {
            match program_of(segment) {
                Ok(Some(name)) => {
                    if !names.iter().any(|n| n == &name) {
                        names.push(name);
                    }
                }
                Ok(None) => {}
                Err(why) => {
                    beyond.get_or_insert(why);
                }
            }
        }
        if names.is_empty() {
            beyond.get_or_insert(NOTHING);
        }
        Programs { names, beyond }
    }
}

/// Steps over the word after a redirection. Writing into the bit bucket
/// reaches nothing; writing anywhere else is a file write.
fn redirect_target(chars: &[char], mut i: usize, beyond: &mut Option<&'static str>) -> usize {
    while matches!(chars.get(i), Some(c) if *c == ' ' || *c == '\t') {
        i += 1;
    }
    let start = i;
    while matches!(chars.get(i), Some(c) if !c.is_whitespace() && !"|;&<>()".contains(*c)) {
        i += 1;
    }
    let target: String = chars[start..i].iter().collect();
    if target != "/dev/null" {
        beyond.get_or_insert(REDIRECTS);
    }
    i
}

/// The program one segment runs, or `None` for a segment that only assigns.
fn program_of(words: &[String]) -> Result<Option<String>, &'static str> {
    let mut at = 0;
    loop {
        // `VAR=value` prefixes and `!` are not the program.
        while matches!(words.get(at), Some(w) if w.contains('=') || w == "!") {
            at += 1;
        }
        let Some(program) = words.get(at) else {
            return Ok(None);
        };
        if program.starts_with('$') {
            return Err(VARIABLE);
        }
        let name = bare_name(program);
        if WRAPPERS.contains(&name.as_str()) {
            return Err(HANDS_OVER);
        }
        if let Some((_, own)) = PASS_THROUGH.iter().find(|(p, _)| *p == name) {
            // Keyed by the program after the proxy, when there is one.
            at += 1;
            if matches!(words.get(at), Some(w) if own.contains(&w.as_str())) {
                at += 1;
            }
            if words.get(at).is_none() {
                return Ok(Some(program.clone()));
            }
            continue;
        }
        let rest = &words[at + 1..];
        if let Some((_, shorts, longs)) = INLINE_FLAGS.iter().find(|(p, _, _)| *p == name) {
            // Only the interpreter's own flags count: a `-c` after the script
            // name belongs to the script.
            let mut flags = rest
                .iter()
                .take_while(|w| w.starts_with('-') && w.as_str() != "--");
            let subcommand = rest.iter().find(|w| !w.starts_with('-'));
            let inline = flags.any(|w| {
                longs.contains(&w.as_str())
                    || (!w.starts_with("--") && w.chars().skip(1).any(|c| shorts.contains(&c)))
            }) || subcommand.is_some_and(|w| longs.contains(&w.as_str()));
            if inline {
                return Err(INLINE_CODE);
            }
        }
        if name == "find" && rest.iter().any(|w| FIND_RUNS.contains(&w.as_str())) {
            return Err(HANDS_OVER);
        }
        return Ok(Some(program.clone()));
    }
}

/// `program` without its directory or `.exe`, lowercased, for matching
/// against the lists above.
fn bare_name(program: &str) -> String {
    let lower = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    lower
        .strip_suffix(".exe")
        .map(str::to_owned)
        .unwrap_or(lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(command: &str) -> Vec<String> {
        let programs = Programs::of(command);
        assert_eq!(programs.beyond, None, "{command:?}: {:?}", programs.beyond);
        programs.names
    }

    fn beyond(command: &str) -> &'static str {
        Programs::of(command)
            .beyond
            .unwrap_or_else(|| panic!("{command:?} passed as coverable"))
    }

    #[test]
    fn a_command_that_runs_one_program_names_it() {
        for (plain, program) in [
            ("git status", "git"),
            ("cargo test 2>&1", "cargo"),
            ("cargo build 2>/dev/null", "cargo"),
            ("cargo build >/dev/null 2>&1", "cargo"),
            ("cargo build &>/dev/null", "cargo"),
            ("FOO=1 git log --oneline", "git"),
            ("npm run build >&2", "npm"),
            ("  ls -la", "ls"),
            ("./venv/bin/python script.py", "./venv/bin/python"),
            ("! grep -q foo file", "grep"),
            ("cargo test < input.txt", "cargo"),
        ] {
            assert_eq!(names(plain), vec![program], "{plain}");
        }
    }

    #[test]
    fn a_chained_command_names_every_program_once() {
        assert_eq!(
            names("cd crates/x && cargo test 2>&1 | tail -20"),
            ["cd", "cargo", "tail"]
        );
        assert_eq!(names("git status; git log || true"), ["git", "true"]);
        assert_eq!(names("sleep 100 &"), ["sleep"]);
        assert_eq!(names("git status\nrm -rf ~"), ["git", "rm"]);
        assert_eq!(names("(cd x && make) && ls"), ["cd", "make", "ls"]);
        assert_eq!(names("{ a; b; }"), ["a", "b"]);
        assert_eq!(names("cargo test |& tee"), ["cargo", "tee"]);
    }

    #[test]
    fn an_operator_inside_quotes_is_text() {
        assert_eq!(names(r#"git commit -m "fix; tidy""#), ["git"]);
        assert_eq!(names("grep 'a|b' file"), ["grep"]);
        assert_eq!(names(r#"echo "a > b""#), ["echo"]);
        assert_eq!(names(r"echo a\;b"), ["echo"]);
        assert_eq!(names(r#"git commit -m "say \"hi\"; done""#), ["git"]);
        assert_eq!(names("echo 'it''s' && ls"), ["echo", "ls"]);
    }

    #[test]
    fn a_command_that_does_more_says_what() {
        for (line, reason) in [
            ("git log | sh", "hands"),
            ("git show > ~/.bashrc", "file"),
            ("git show >> log.txt", "file"),
            ("git show &> out.txt", "file"),
            ("git show >&out.txt", "file"),
            ("git log $(rm -rf ~)", "inside"),
            ("git log `rm -rf ~`", "inside"),
            (r#"echo "$(rm -rf ~)""#, "inside"),
            ("diff <(ls a) <(ls b)", "inside"),
            ("sh -c 'rm -rf ~'", "hands"),
            ("/usr/bin/env rm -rf ~", "hands"),
            ("sudo rm -rf /", "hands"),
            ("pwsh.exe -c Remove-Item", "hands"),
            ("ls && sudo rm -rf /", "hands"),
            ("cat <<EOF\nhello\nEOF", "script"),
            ("python3 <<< 'print(1)'", "script"),
            ("python3 -c 'import os'", "code"),
            ("python3 -uc 'import os'", "code"),
            ("node -e 'require(\"fs\")'", "code"),
            ("node --eval 1", "code"),
            ("perl -E 'say 1'", "code"),
            (r"find . -name '*.rs' -exec rm {} \;", "hands"),
            ("find . -execdir rm {} +", "hands"),
            ("$CARGO test", "variable"),
            ("${CARGO} test", "variable"),
            ("", "no program"),
            ("FOO=1", "no program"),
        ] {
            assert!(beyond(line).contains(reason), "{line:?}: {}", beyond(line));
        }
    }

    #[test]
    fn a_substituted_command_is_still_named_for_a_hook() {
        // A grant may not use these names, but a hook on `rm` should fire.
        let programs = Programs::of("git log $(rm -rf ~)");
        assert_eq!(programs.names, ["git", "rm"]);
        assert_eq!(programs.beyond, Some(SUBSTITUTES));
    }

    #[test]
    fn the_first_reason_is_the_one_reported() {
        assert_eq!(Programs::of("git show > f | sh").beyond, Some(REDIRECTS));
    }

    #[test]
    fn a_pass_through_proxy_is_keyed_by_what_it_runs() {
        assert_eq!(names("rtk cargo test"), ["cargo"]);
        assert_eq!(names("rtk proxy git status"), ["git"]);
        assert_eq!(names("rtk gain"), ["gain"]);
        assert_eq!(names("rtk"), ["rtk"]);
        assert_eq!(beyond("rtk sh -c ls"), HANDS_OVER);
        assert_eq!(names("rtk git add . && rtk git commit -m x"), ["git"]);
    }

    #[test]
    fn an_interpreter_running_a_file_is_an_ordinary_program() {
        assert_eq!(names("python3 script.py -c"), ["python3"]);
        assert_eq!(names("python3 -m pytest --co"), ["python3"]);
        assert_eq!(names("node script.js --port 3000"), ["node"]);
        assert_eq!(beyond("deno eval 1"), INLINE_CODE);
        assert_eq!(names("deno run main.ts"), ["deno"]);
        assert_eq!(names("find . -name '*.rs' -delete"), ["find"]);
    }

    #[test]
    fn a_wrapper_is_matched_by_its_bare_name() {
        assert_eq!(bare_name("/usr/bin/env"), "env");
        assert_eq!(bare_name("C:\\Windows\\System32\\cmd.EXE"), "cmd");
        assert_eq!(bare_name("git"), "git");
    }
}
