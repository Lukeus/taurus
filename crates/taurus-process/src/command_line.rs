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
//!
//! The line is read by the rules of the shell that will run it. `cmd.exe` is
//! not a POSIX shell with different programs: a single quote quotes nothing, a
//! backslash escapes nothing, and `%VAR%` is pasted in before the line is read
//! at all, so reading its lines by `sh`'s rules names the wrong programs.

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
const EXPANDS: &str = "pastes a variable into the line before the line is read";

/// Whose rules a line is read by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialect {
    Posix,
    Cmd,
}

impl Dialect {
    /// The shell `run_command` starts on this platform.
    const HOST: Self = if cfg!(windows) {
        Self::Cmd
    } else {
        Self::Posix
    };
}

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

/// What `cmd.exe` adds to [`WRAPPERS`]. `for` and `if` are built in, and each
/// runs the command written after it.
const CMD_WRAPPERS: &[&str] = &[
    "start", "call", "for", "if", "runas", "forfiles", "wsl", "rundll32", "mshta",
];

/// The wrappers that can also run one script file and nothing else, which is
/// what `python3 script.py` does and is as grantable.
const POWERSHELL: &[&str] = &["powershell", "pwsh"];

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
        Self::read(command, Dialect::HOST)
    }

    fn read(command: &str, dialect: Dialect) -> Self {
        let chars: Vec<char> = command.chars().collect();
        let mut segments: Vec<Vec<String>> = Vec::new();
        let mut words: Vec<String> = Vec::new();
        let mut word = String::new();
        let mut in_word = false;
        let mut beyond: Option<&'static str> =
            (dialect == Dialect::Cmd && expands_a_variable(&chars)).then_some(EXPANDS);
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
                    // `cmd.exe` has no escape inside quotes, and a line end
                    // ends the quote.
                    '\n' | '\r' if dialect == Dialect::Cmd => {
                        double = false;
                        end_segment(&mut segments, &mut words, &mut word, &mut in_word);
                    }
                    '\\' if dialect == Dialect::Posix
                        && matches!(next, Some('"' | '\\' | '$' | '`')) =>
                    {
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
                '\'' if dialect == Dialect::Posix => {
                    single = true;
                    in_word = true;
                }
                '"' => {
                    double = true;
                    in_word = true;
                }
                // The same job as a POSIX backslash, and the same trick: `c^md`
                // is `cmd`.
                '^' if dialect == Dialect::Cmd => {
                    match next {
                        Some('\n') | None => {}
                        Some(n) => {
                            word.push(n);
                            in_word = true;
                        }
                    }
                    i += 1;
                }
                '\\' if dialect == Dialect::Posix => {
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
                    i = redirect_target(&chars, i, dialect, &mut beyond);
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
                    i = redirect_target(&chars, i, dialect, &mut beyond);
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
            match program_of(segment, dialect) {
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
fn redirect_target(
    chars: &[char],
    mut i: usize,
    dialect: Dialect,
    beyond: &mut Option<&'static str>,
) -> usize {
    while matches!(chars.get(i), Some(c) if *c == ' ' || *c == '\t') {
        i += 1;
    }
    let start = i;
    while matches!(chars.get(i), Some(c) if !c.is_whitespace() && !"|;&<>()".contains(*c)) {
        i += 1;
    }
    let target: String = chars[start..i].iter().collect();
    // Each shell's bit bucket is an ordinary file name to the other.
    let discards = match dialect {
        Dialect::Posix => target == "/dev/null",
        Dialect::Cmd => target.eq_ignore_ascii_case("nul"),
    };
    if !discards {
        beyond.get_or_insert(REDIRECTS);
    }
    i
}

/// Whether a line holds `%NAME%`. Conservative on purpose: `cmd.exe` expands it
/// inside quotes too and its value can hold `&`, and `%H %s` in a `git log`
/// format reads the same as a variable with a space in its name.
fn expands_a_variable(chars: &[char]) -> bool {
    chars.split(|c| matches!(c, '\n' | '\r')).any(|line| {
        line.split(|c| *c == '%')
            .skip(1)
            .take(line.iter().filter(|c| **c == '%').count().saturating_sub(1))
            .any(|between| !between.is_empty())
    })
}

/// Whether PowerShell's arguments run one script file and nothing else.
///
/// Spelled out flag by flag, because everything else on this command line is
/// a way to run code from the line: `-Command`, `-EncodedCommand`, and a bare
/// argument, which `powershell` takes as a command and `pwsh` as a file.
fn runs_one_file(arguments: &[String]) -> bool {
    let mut arguments = arguments.iter();
    while let Some(flag) = arguments.next() {
        match flag.to_ascii_lowercase().as_str() {
            "-noprofile" | "-nop" | "-nologo" | "-noninteractive" | "-noni" => {}
            "-executionpolicy" | "-ep" | "-ex" => {
                arguments.next();
            }
            // `-File -` reads the script from stdin.
            "-file" | "-f" => return arguments.next().is_some_and(|f| !f.starts_with('-')),
            _ => return false,
        }
    }
    false
}

/// Whether `program` only hands the rest of its line to another program.
fn is_wrapper(program: &str, dialect: Dialect) -> bool {
    let wrapper = |name: &str| {
        WRAPPERS.contains(&name) || (dialect == Dialect::Cmd && CMD_WRAPPERS.contains(&name))
    };
    wrapper(&bare_name(program))
        || (dialect == Dialect::Cmd && {
            // `cmd/c del x` needs no space, and `cmd.` is still `cmd`.
            let before_switch = program.split('/').next().unwrap_or(program);
            wrapper(bare_name(before_switch).trim_end_matches('.'))
        })
}

/// The program one segment runs, or `None` for a segment that only assigns.
fn program_of(words: &[String], dialect: Dialect) -> Result<Option<String>, &'static str> {
    let mut at = 0;
    loop {
        // `VAR=value` prefixes and `!` are not the program. `cmd.exe` has
        // neither: `del=x git status` runs `del`, because `=` separates words
        // there, as `,` and `;` do, and a leading `@` only silences the echo.
        if dialect == Dialect::Posix {
            while matches!(words.get(at), Some(w) if w.contains('=') || w == "!") {
                at += 1;
            }
        }
        let Some(word) = words.get(at) else {
            return Ok(None);
        };
        let program = match dialect {
            Dialect::Posix => word.as_str(),
            Dialect::Cmd => word
                .trim_start_matches(['@', ',', ';', '='])
                .split([',', ';', '='])
                .next()
                .unwrap_or_default(),
        };
        if program.is_empty() {
            at += 1;
            continue;
        }
        if program.starts_with('$') {
            return Err(VARIABLE);
        }
        let name = bare_name(program);
        if POWERSHELL.contains(&name.as_str()) && runs_one_file(&words[at + 1..]) {
            return Ok(Some(program.to_owned()));
        }
        if is_wrapper(program, dialect) {
            return Err(HANDS_OVER);
        }
        if let Some((_, own)) = PASS_THROUGH.iter().find(|(p, _)| *p == name) {
            // Keyed by the program after the proxy, when there is one.
            at += 1;
            if matches!(words.get(at), Some(w) if own.contains(&w.as_str())) {
                at += 1;
            }
            if words.get(at).is_none() {
                return Ok(Some(program.to_owned()));
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
        return Ok(Some(program.to_owned()));
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
        let programs = Programs::read(command, Dialect::Posix);
        assert_eq!(programs.beyond, None, "{command:?}: {:?}", programs.beyond);
        programs.names
    }

    fn beyond(command: &str) -> &'static str {
        Programs::read(command, Dialect::Posix)
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
        let programs = Programs::read("git log $(rm -rf ~)", Dialect::Posix);
        assert_eq!(programs.names, ["git", "rm"]);
        assert_eq!(programs.beyond, Some(SUBSTITUTES));
    }

    #[test]
    fn the_first_reason_is_the_one_reported() {
        assert_eq!(
            Programs::read("git show > f | sh", Dialect::Posix).beyond,
            Some(REDIRECTS)
        );
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

    fn cmd(command: &str) -> Programs {
        Programs::read(command, Dialect::Cmd)
    }

    #[test]
    fn powershell_running_one_file_is_an_ordinary_program() {
        for dialect in [Dialect::Posix, Dialect::Cmd] {
            for (line, program) in [
                ("pwsh -File build.ps1", "pwsh"),
                ("pwsh -f build.ps1 -Command x", "pwsh"),
                (
                    "powershell.exe -NoProfile -File build.ps1",
                    "powershell.exe",
                ),
                (
                    "powershell -nop -ExecutionPolicy Bypass -File build.ps1",
                    "powershell",
                ),
            ] {
                let programs = Programs::read(line, dialect);
                assert_eq!(programs.beyond, None, "{line}");
                assert_eq!(programs.names, [program], "{line}");
            }
            for line in [
                r#"powershell -Command "Get-ChildItem src""#,
                "pwsh -c Remove-Item x",
                "powershell -EncodedCommand ZABpAHIA",
                "powershell Remove-Item",
                "pwsh build.ps1",
                "pwsh",
                "pwsh -File",
                "pwsh -File -",
                "pwsh -NoProfile -Command x -File build.ps1",
            ] {
                assert_eq!(
                    Programs::read(line, dialect).beyond,
                    Some(HANDS_OVER),
                    "{line}"
                );
            }
        }
    }

    #[test]
    fn cmd_throws_output_away_into_nul() {
        for line in [
            "cargo test 2>nul",
            "cargo build >NUL 2>&1",
            "cargo test > nul",
        ] {
            let programs = cmd(line);
            assert_eq!(programs.beyond, None, "{line}");
            assert_eq!(programs.names, ["cargo"], "{line}");
        }
        // Each bit bucket is a file to the other shell.
        assert_eq!(cmd("cargo test 2>/dev/null").beyond, Some(REDIRECTS));
        assert_eq!(cmd("cargo test >nul.txt").beyond, Some(REDIRECTS));
        assert_eq!(
            Programs::read("cargo test 2>nul", Dialect::Posix).beyond,
            Some(REDIRECTS)
        );
    }

    #[test]
    fn cmd_does_not_quote_with_a_single_quote() {
        let programs = cmd("git log 'x & del y'");
        assert_eq!(programs.names, ["git", "del"]);
        assert_eq!(programs.beyond, None);
        // A double quote is still a quote, and a backslash does not end it
        // early or keep it open.
        assert_eq!(cmd(r#"git commit -m "a & b""#).names, ["git"]);
        assert_eq!(cmd(r#"git log "x\" & del y"#).names, ["git", "del"]);
    }

    #[test]
    fn cmd_keeps_a_backslash_in_a_path() {
        for (line, program) in [
            (
                r".\venv\Scripts\python.exe script.py",
                r".\venv\Scripts\python.exe",
            ),
            (r"C:\Tools\foo.exe --version", r"C:\Tools\foo.exe"),
            (
                r#""C:\Program Files\x\y.exe" a"#,
                r"C:\Program Files\x\y.exe",
            ),
        ] {
            assert_eq!(cmd(line).names, [program], "{line}");
        }
        assert_eq!(cmd(r"cd /d C:\repo && cargo test").names, ["cd", "cargo"]);
    }

    #[test]
    fn cmd_caret_escapes_and_hides_nothing() {
        assert_eq!(cmd("echo a^&del y").names, ["echo"]);
        assert_eq!(cmd("g^it status").names, ["git"]);
        assert_eq!(cmd("c^md /c del y").beyond, Some(HANDS_OVER));
    }

    #[test]
    fn cmd_expands_a_variable_before_it_reads_the_line() {
        for line in ["git log %X%", r#"git log "%X%""#, "%COMSPEC% /c del y"] {
            assert_eq!(cmd(line).beyond, Some(EXPANDS), "{line}");
        }
        for line in ["git log --format=%H", "echo 100%", "echo %%"] {
            assert_eq!(cmd(line).beyond, None, "{line}");
        }
        // Only `cmd.exe` does this.
        assert_eq!(Programs::read("git log %X%", Dialect::Posix).beyond, None);
    }

    #[test]
    fn cmd_separates_words_where_a_posix_shell_does_not() {
        // Read as an assignment, this would be keyed by `git` and run `del`.
        assert_eq!(cmd("del=x git status").names, ["del"]);
        assert_eq!(cmd(",del x").names, ["del"]);
        assert_eq!(cmd("@del x").names, ["del"]);
        for line in [
            "@cmd /c del y",
            "cmd,/c del y",
            "cmd/c del y",
            "cmd. /c del y",
        ] {
            assert_eq!(cmd(line).beyond, Some(HANDS_OVER), "{line}");
        }
    }

    #[test]
    fn cmd_built_ins_that_run_a_command_are_wrappers() {
        for line in [
            "for %i in (a) do del x",
            "if exist x del y",
            "call x.bat",
            "start del",
        ] {
            assert_eq!(cmd(line).beyond, Some(HANDS_OVER), "{line}");
        }
        // Only there: `start` is an ordinary name to a POSIX shell.
        assert_eq!(names("start server"), ["start"]);
    }

    #[test]
    fn a_folder_named_for_a_wrapper_is_not_one() {
        assert_eq!(
            cmd(r".\env\Scripts\python.exe x.py").names,
            [r".\env\Scripts\python.exe"]
        );
    }
}
