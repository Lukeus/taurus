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
const COMMENT: &str = "has a shell comment in it, which this check doesn't read";
const QUOTING: &str = "quotes text in a way this check doesn't read";
const OPEN_QUOTE: &str = "leaves a quote open";
const BLANKS: &str = "separates words with a character the shell doesn't treat as a space";
const EXPANSION: &str = "uses a shell expansion this check doesn't read";
const BRACES: &str = "builds its program's name with brace expansion";
const WILDCARD: &str = "names its program with a wildcard";
const CONTROL_FLOW: &str = "uses the shell's own control flow, which runs commands no program \
                            name stands for";
const NOT_A_SETTING: &str = "runs something with `=` in its name, which reads like a setting";
const ASSIGNS: &str = "sets a variable that can change what the command runs";

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
    "setsid",
    "ionice",
    "flock",
    "script",
    "chroot",
    "unshare",
    "busybox",
    "toybox",
    "parallel",
    "caffeinate",
    "arch",
    "pkexec",
    // Built in, and each one runs or redefines a command later on.
    "trap",
    "alias",
    "hash",
    "enable",
];

/// Words that are the shell's own syntax where a program would go. A grant for
/// `do` would cover every loop's body: `for f in a; do rm -rf ~; done` runs
/// `rm`, and no segment of it starts with that.
const RESERVED: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "select", "function", "coproc",
];

/// Built-ins that set variables from their arguments.
const DECLARES: &[&str] = &["export", "declare", "typeset", "readonly", "local"];

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
/// matched inside a cluster too, so `-uc` counts. A version on the end of the
/// name is ignored, so `python3.12` counts as well. See [`runs_inline_code`].
const INLINE_FLAGS: &[(&str, &[char], &[&str])] = &[
    ("python", &['c'], &[]),
    ("python2", &['c'], &[]),
    ("python3", &['c'], &[]),
    ("node", &['e', 'p'], &["--eval", "--print"]),
    ("nodejs", &['e', 'p'], &["--eval", "--print"]),
    ("deno", &[], &["eval"]),
    ("ruby", &['e'], &[]),
    ("perl", &['e', 'E'], &[]),
    ("php", &['r'], &["--run"]),
    ("pypy", &['c'], &[]),
    ("bun", &['e', 'p'], &["--eval", "--print"]),
    ("lua", &['e'], &[]),
    ("osascript", &['e'], &[]),
];

/// `find` actions that run a program on what was found.
const FIND_RUNS: &[&str] = &["-exec", "-execdir", "-ok", "-okdir"];

/// `find` actions that write what was found into a file.
const FIND_WRITES: &[&str] = &["-fprint", "-fprint0", "-fprintf", "-fls"];

impl Programs {
    /// Takes `command` apart and names what it runs.
    pub fn of(command: &str) -> Self {
        Self::read(command, Dialect::HOST)
    }

    fn read(command: &str, dialect: Dialect) -> Self {
        let chars: Vec<char> = command.chars().collect();
        let mut s = Scan::default();
        let mut beyond: Option<&'static str> =
            (dialect == Dialect::Cmd && expands_a_variable(&chars)).then_some(EXPANDS);
        let (mut single, mut double) = (false, false);
        let posix = dialect == Dialect::Posix;

        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if single {
                if c == '\'' {
                    single = false;
                } else {
                    s.word.push(c);
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
                        s.end_segment();
                    }
                    '\\' if posix && matches!(next, Some('"' | '\\' | '$' | '`')) => {
                        s.word.push(next.unwrap_or_default());
                        i += 1;
                    }
                    '$' if posix && next == Some('{') => {
                        i = s.parameter(&chars, i, &mut beyond);
                        continue;
                    }
                    // Substitution happens inside double quotes too.
                    '$' if next == Some('(') => {
                        beyond.get_or_insert(SUBSTITUTES);
                        s.end_segment();
                        i += 1;
                    }
                    '`' => {
                        beyond.get_or_insert(SUBSTITUTES);
                        s.end_segment();
                    }
                    _ => s.word.push(c),
                }
                i += 1;
                continue;
            }
            match c {
                '\'' if posix => {
                    single = true;
                    s.in_word = true;
                    s.mark_quoted();
                }
                '"' => {
                    double = true;
                    s.in_word = true;
                    s.mark_quoted();
                }
                // The same job as a POSIX backslash, and the same trick: `c^md`
                // is `cmd`.
                '^' if dialect == Dialect::Cmd => {
                    match next {
                        Some('\n') | None => {}
                        // A line ending in CRLF continues the line too.
                        Some('\r') if chars.get(i + 2) == Some(&'\n') => i += 1,
                        Some(n) => {
                            s.word.push(n);
                            s.in_word = true;
                        }
                    }
                    i += 1;
                }
                '\\' if posix => {
                    // A backslash-newline continues the line; anything else
                    // escaped is literal.
                    match next {
                        Some('\n') => {}
                        Some(n) => {
                            s.mark_quoted();
                            s.word.push(n);
                            s.in_word = true;
                        }
                        None => {}
                    }
                    i += 1;
                }
                // `$'…'` has escapes a plain single quote does not, so `\'`
                // inside one does not end it, and reading it as a plain quote
                // loses track of everything after. `$"…"` isn't read either.
                '$' if posix && matches!(next, Some('\'' | '"')) => {
                    beyond.get_or_insert(QUOTING);
                    s.word.push(c);
                    s.in_word = true;
                }
                '$' if posix && next == Some('{') => {
                    s.in_word = true;
                    i = s.parameter(&chars, i, &mut beyond);
                    continue;
                }
                '$' if next == Some('(') => {
                    beyond.get_or_insert(SUBSTITUTES);
                    s.end_segment();
                    i += 1;
                }
                '`' => {
                    beyond.get_or_insert(SUBSTITUTES);
                    s.end_segment();
                }
                '<' | '>' if next == Some('(') => {
                    beyond.get_or_insert(SUBSTITUTES);
                    s.end_segment();
                    i += 1;
                }
                '<' if next == Some('<') => {
                    beyond.get_or_insert(HEREDOC);
                    s.end_word();
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
                    s.end_word();
                    i = redirect_target(&chars, i, dialect, &mut beyond);
                    continue;
                }
                '>' => {
                    // The fd digits before it (`2>`) are not a word.
                    if s.in_word && !s.word.is_empty() && s.word.chars().all(|d| d.is_ascii_digit())
                    {
                        s.word.clear();
                        s.in_word = false;
                    }
                    s.end_word();
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
                    s.end_segment();
                    if matches!(next, Some('|' | '&')) {
                        i += 1;
                    }
                }
                ';' => {
                    s.end_segment();
                    if next == Some(';') {
                        i += 1;
                    }
                }
                '&' => {
                    s.end_segment();
                    if next == Some('&') {
                        i += 1;
                    }
                }
                '\n' | '\r' | '(' | ')' => s.end_segment(),
                // A `{` standing alone opens a group. Glued to anything where
                // the program goes it's brace expansion, and `git{x,y}` runs
                // `gitx`.
                '{' => {
                    if posix
                        && s.words.len() == s.leading
                        && (s.in_word || next.is_some_and(|n| !n.is_whitespace()))
                    {
                        beyond.get_or_insert(BRACES);
                    }
                    s.end_word();
                }
                '}' => s.end_word(),
                // A comment ends at the line's end and nothing in it is read,
                // so a quote in one (`# it's`) must not open anything here.
                // Rather than read comments too, a line with one is asked about.
                '#' if posix && !s.in_word => {
                    beyond.get_or_insert(COMMENT);
                    s.word.push(c);
                    s.in_word = true;
                }
                c if c.is_whitespace() => {
                    // A shell splits on a space and a tab. A no-break space
                    // or a form feed is part of the word to it, and not here.
                    if c != ' ' && c != '\t' {
                        beyond.get_or_insert(BLANKS);
                    }
                    s.end_word();
                }
                _ => {
                    s.word.push(c);
                    s.in_word = true;
                }
            }
            i += 1;
        }
        // A quote still open means the two of us disagree about the line.
        // `cmd.exe` ends a quote at the end of the line, so it's fine there.
        if posix && (single || double) {
            beyond.get_or_insert(OPEN_QUOTE);
        }
        s.end_segment();

        let mut names: Vec<String> = Vec::new();
        for segment in &s.segments {
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

/// One command of a line, as words.
#[derive(Default)]
struct Segment {
    words: Vec<String>,
    /// How many of `words` come before the program: `VAR=value` settings and
    /// `!`. A word with a quote before its `=` isn't among them, because the
    /// shell runs `'a'=b` as a program called `a=b`.
    leading: usize,
}

/// A line being taken apart.
#[derive(Default)]
struct Scan {
    segments: Vec<Segment>,
    words: Vec<String>,
    word: String,
    in_word: bool,
    leading: usize,
    /// The word so far has a quote or an escape ahead of any `=` in it.
    quoted_name: bool,
}

impl Scan {
    fn mark_quoted(&mut self) {
        if !self.word.contains('=') {
            self.quoted_name = true;
        }
    }

    fn end_word(&mut self) {
        if self.in_word {
            let word = std::mem::take(&mut self.word);
            if self.words.len() == self.leading
                && (word == "!" || (!self.quoted_name && is_assignment(&word)))
            {
                self.leading += 1;
            }
            self.words.push(word);
            self.in_word = false;
        }
        self.quoted_name = false;
    }

    fn end_segment(&mut self) {
        self.end_word();
        if !self.words.is_empty() {
            self.segments.push(Segment {
                words: std::mem::take(&mut self.words),
                leading: std::mem::take(&mut self.leading),
            });
        }
    }

    /// Reads the `${` at `at` into the word and returns where to go on from.
    ///
    /// `${NAME}` is a plain variable. Anything else in there has quoting and
    /// substitution rules of its own, so it's asked about rather than read.
    fn parameter(&mut self, chars: &[char], at: usize, beyond: &mut Option<&'static str>) -> usize {
        let name = chars[at + 2..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
            .count();
        let close = at + 2 + name;
        if name > 0 && chars.get(close) == Some(&'}') {
            self.word.extend(&chars[at..=close]);
            return close + 1;
        }
        beyond.get_or_insert(EXPANSION);
        self.word.push_str("${");
        at + 2
    }
}

/// Whether the shell reads `word` as setting a variable.
fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let name = name.strip_suffix('+').unwrap_or(name);
    !name.starts_with(|c: char| c.is_ascii_digit())
        && !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Variables that tune how a program reports or behaves, and whose value is
/// never a program or a path it would load.
const QUIET_NAMES: &[&str] = &[
    "RUST_BACKTRACE",
    "RUST_LOG",
    "RUST_TEST_THREADS",
    "RUST_MIN_STACK",
    "CARGO_TERM_COLOR",
    "CARGO_INCREMENTAL",
    "NO_COLOR",
    "FORCE_COLOR",
    "CLICOLOR",
    "CI",
    "TZ",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "NODE_ENV",
    "DEBUG",
    "TERM",
    "COLUMNS",
    "LINES",
    "PYTHONUNBUFFERED",
    "PYTHONDONTWRITEBYTECODE",
];

/// Whether a `VAR=value` setting leaves the program it's on being that program.
///
/// `PATH=./evil git status` isn't `git`, and neither is `GIT_PAGER='rm -rf ~'
/// git log`. Nobody can list every variable some program runs something from
/// (`RUSTC_WRAPPER`, `NODE_OPTIONS`, `CC`), so this lists what is safe: a known
/// quiet name with a plain value, or any other name with a value that's only a
/// number or a switch, since `1` can't carry a command. A name that holds a
/// place to look something up still doesn't pass, because `PATH=1` is a
/// directory called `1`.
fn is_quiet_assignment(word: &str) -> bool {
    let Some((name, value)) = word.split_once('=') else {
        return false;
    };
    let name = name.strip_suffix('+').unwrap_or(name);
    if QUIET_NAMES.contains(&name) {
        return value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.,:=+-*".contains(c));
    }
    let switch = value.chars().all(|c| c.is_ascii_digit())
        || ["true", "false", "on", "off", "yes", "no"]
            .iter()
            .any(|word| value.eq_ignore_ascii_case(word));
    let upper = name.to_ascii_uppercase();
    let places = ["PATH", "HOME", "CONFIG", "DIR", "FILE"]
        .iter()
        .any(|part| upper.contains(part))
        || upper.ends_with("ENV")
        || ["LD_", "DYLD_", "GIT_"]
            .iter()
            .any(|prefix| upper.starts_with(prefix));
    switch && !places
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
fn program_of(segment: &Segment, dialect: Dialect) -> Result<Option<String>, &'static str> {
    let words = &segment.words;
    let posix = dialect == Dialect::Posix;
    let mut at = 0;
    // `VAR=value` prefixes and `!` are not the program, and a line of nothing
    // but a setting is checked too: `PATH=./evil; git status` keeps it for the
    // next command. `cmd.exe` has neither: `del=x git status` runs `del`,
    // because `=` separates words there, as `,` and `;` do, and a leading `@`
    // only silences the echo.
    if posix {
        while at < segment.leading {
            if words[at] != "!" && !is_quiet_assignment(&words[at]) {
                return Err(ASSIGNS);
            }
            at += 1;
        }
    }
    loop {
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
        if program.starts_with('$') || (posix && program.contains('$')) {
            return Err(VARIABLE);
        }
        if posix {
            if RESERVED.contains(&program) {
                return Err(CONTROL_FLOW);
            }
            // Not a setting, or it would have been stepped over: `./a=b git`
            // runs `./a=b`.
            if program.contains('=') {
                return Err(NOT_A_SETTING);
            }
            if program != "[" && program != "[[" && program.contains(['*', '?', '[']) {
                return Err(WILDCARD);
            }
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
        if runs_inline_code(&name, rest) {
            return Err(INLINE_CODE);
        }
        if name == "find" {
            if rest.iter().any(|w| FIND_RUNS.contains(&w.as_str())) {
                return Err(HANDS_OVER);
            }
            if rest.iter().any(|w| FIND_WRITES.contains(&w.as_str())) {
                return Err(REDIRECTS);
            }
        }
        // `export PATH=./evil; git status` by another spelling.
        if posix
            && DECLARES.contains(&name.as_str())
            && rest
                .iter()
                .any(|w| w.contains('=') && !is_quiet_assignment(w))
        {
            return Err(ASSIGNS);
        }
        return Ok(Some(program.to_owned()));
    }
}

/// Whether an interpreter's arguments run code written into the line.
///
/// Only the interpreter's own flags count, since a `-c` after the script name
/// belongs to the script. Telling the two apart needs each interpreter's list
/// of flags that take a value, so this doesn't try: a bare word right after a
/// flag might be that flag's value, and the look goes on past it. `python3 -W
/// ignore -c x` is caught that way, and `python3 -u script.py -c x` gets asked
/// about, which is the side to be wrong on. A bare word with no flag before it
/// is the script, and ends the look.
fn runs_inline_code(name: &str, rest: &[String]) -> bool {
    // `python3.12` is `python3`.
    let versionless = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    let Some((_, shorts, longs)) = INLINE_FLAGS
        .iter()
        .find(|(p, _, _)| *p == name || *p == versionless)
    else {
        return false;
    };
    // `deno eval`.
    if rest
        .iter()
        .find(|w| !w.starts_with('-'))
        .is_some_and(|w| longs.contains(&w.as_str()))
    {
        return true;
    }
    let mut after_a_flag = false;
    for word in rest {
        if word == "--" {
            return false;
        }
        if word.len() > 1 && word.starts_with('-') {
            // `--eval=code` is `--eval code`.
            let long = word.split('=').next().unwrap_or(word);
            if longs.contains(&long)
                || (!word.starts_with("--") && word.chars().skip(1).any(|c| shorts.contains(&c)))
            {
                return true;
            }
            // Everything after `-m` belongs to the module.
            if word == "-m" && versionless.starts_with("python") {
                return false;
            }
            after_a_flag = true;
        } else {
            if !after_a_flag {
                return false;
            }
            after_a_flag = false;
        }
    }
    false
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

    /// Runs a line with `Programs::read`, for the lines that must be asked about.
    fn reason(line: &str) -> Option<&'static str> {
        Programs::read(line, Dialect::Posix).beyond
    }

    #[test]
    fn a_comment_cannot_open_a_quote_that_hides_the_next_line() {
        for line in [
            "git status # it's\nrm -rf ~ # '",
            "# run the tests\ncargo test",
            "git status #\nrm",
        ] {
            assert_eq!(reason(line), Some(COMMENT), "{line:?}");
        }
        // Not a comment: `#` inside a word, or quoted, or escaped.
        assert_eq!(names("git log foo#bar"), ["git"]);
        assert_eq!(names("git commit -m '# a'"), ["git"]);
        assert_eq!(names(r"echo \#"), ["echo"]);
        assert_eq!(cmd("git log # x").beyond, None);
    }

    #[test]
    fn ansi_quoting_is_not_read_as_a_plain_quote() {
        assert_eq!(reason(r"git log $'\''; rm -rf ~ #'"), Some(QUOTING));
        assert_eq!(reason(r#"echo $"hi""#), Some(QUOTING));
        assert_eq!(names(r#"echo "$'a'""#), ["echo"]);
    }

    #[test]
    fn what_the_scanner_cannot_read_is_asked_about() {
        assert_eq!(reason("git log 'x"), Some(OPEN_QUOTE));
        assert_eq!(reason("git\u{a0}status"), Some(BLANKS));
        assert_eq!(reason("git show >/dev/null\u{a0}x"), Some(BLANKS));
        assert_eq!(reason("git\u{c}x"), Some(BLANKS));
        assert_eq!(
            reason(r#"echo "${x:-"}"; rm -rf ~; "${y:-"}""#),
            Some(EXPANSION)
        );
        assert_eq!(reason("echo ${x:-a}"), Some(EXPANSION));
        assert_eq!(names("echo ${HOME} \"${HOME}\" $HOME"), ["echo"]);
        assert_eq!(reason("git{x,y} status"), Some(BRACES));
        assert_eq!(reason("{rm,-rf,x}"), Some(BRACES));
        assert_eq!(names("echo {a,b} x{1,2}"), ["echo"]);
        assert_eq!(reason("/bin/r? -rf x"), Some(WILDCARD));
        assert_eq!(reason("/bin/$X -rf x"), Some(VARIABLE));
        assert_eq!(reason("git${X} status"), Some(VARIABLE));
        assert_eq!(names("[ -f x ] && [[ -f y ]]"), ["[", "[["]);
    }

    #[test]
    fn a_reserved_word_is_never_a_program_to_grant() {
        for line in [
            "for f in a; do rm -rf ~; done",
            "if true; then rm -rf ~; fi",
            "while true; do cargo test; done",
            "case x in x) rm;; esac",
            "true && do rm",
        ] {
            assert_eq!(reason(line), Some(CONTROL_FLOW), "{line:?}");
        }
        // Part of a name, or an argument, is only that.
        assert_eq!(names("./do x && dot for"), ["./do", "dot"]);
    }

    #[test]
    fn a_word_with_an_equals_sign_is_a_setting_only_if_the_shell_says_so() {
        for line in ["./a=b git status", "'a'=b git status", r"a\=b git status"] {
            assert_eq!(reason(line), Some(NOT_A_SETTING), "{line:?}");
        }
        assert_eq!(names("git log --format=%H"), ["git"]);
        assert_eq!(names("git commit -m 'a=b'"), ["git"]);
        assert_eq!(reason("rtk FOO=1 git"), Some(NOT_A_SETTING));
    }

    #[test]
    fn a_setting_that_can_change_what_runs_is_asked_about() {
        for line in [
            "PATH=./evil git status",
            "PATH=1 git status",
            "GIT_PAGER='rm -rf ~' git log",
            "GIT_PAGER=true git log",
            "LD_PRELOAD=./x.so git status",
            "RUSTC_WRAPPER=./x cargo build",
            "FOO=bar cargo test",
            "BASH_ENV=1 git status",
            "HOME=1 git status",
            // Kept for what comes next.
            "PATH=./evil; git status",
            "PATH=./evil\ngit status",
            "export PATH=./evil; git status",
            "! PATH=./evil git status",
            "RUST_LOG=$(rm) cargo test",
        ] {
            assert!(
                matches!(reason(line), Some(ASSIGNS) | Some(SUBSTITUTES)),
                "{line:?}: {:?}",
                reason(line)
            );
        }
        for (line, program) in [
            ("VAR=1 cargo test", "cargo"),
            ("RUST_BACKTRACE=1 cargo test", "cargo"),
            ("RUST_LOG=taurus::x=debug cargo test", "cargo"),
            ("CARGO_TERM_COLOR=always cargo test", "cargo"),
            ("NODE_ENV=test npm test", "npm"),
            ("CI=true FOO=0 npm test", "npm"),
            ("! FOO=1 git status", "git"),
            ("export FOO=1 && export BAR", "export"),
        ] {
            assert_eq!(names(line), [program], "{line}");
        }
        assert_eq!(names("FOO=1; git status"), ["git"]);
    }

    #[test]
    fn inline_code_is_found_past_a_flag_with_a_value() {
        for line in [
            "python3 -W ignore -c 'import os'",
            "python3 -X utf8 -u -c x",
            "perl -I lib -e 'x'",
            "ruby -I lib -e 'x'",
            "node -r ./x.js -e 'x'",
            "node --eval='x'",
            "node --print=1",
            "python3.12 -c x",
            "pypy3 -c x",
            "bun --eval=x",
            "php --run x",
            "osascript -e x",
            "python3 -u script.py -c x",
        ] {
            assert_eq!(reason(line), Some(INLINE_CODE), "{line:?}");
        }
        for line in [
            "python3 script.py -c x",
            "python3 -m pytest -c cfg.ini",
            "python3 -- -c",
            "node -r ./x.js script.js -e",
            "python3.12 script.py",
        ] {
            assert_eq!(reason(line), None, "{line:?}");
        }
    }

    #[test]
    fn built_ins_and_wrappers_that_run_something_else_hand_over() {
        for line in [
            "trap 'rm -rf ~' EXIT",
            "alias git=rm",
            "hash -p ./evil git",
            "setsid rm -rf x",
            "busybox rm -rf x",
        ] {
            assert_eq!(reason(line), Some(HANDS_OVER), "{line:?}");
        }
    }

    #[test]
    fn find_writing_a_file_is_a_file_write() {
        assert_eq!(reason("find . -fprint out.txt"), Some(REDIRECTS));
        assert_eq!(reason("find . -fls out.txt"), Some(REDIRECTS));
    }

    #[test]
    fn cmd_caret_continues_a_line_ending_in_crlf() {
        assert_eq!(cmd("g^\r\nit status").names, ["git"]);
    }

    #[test]
    fn the_everyday_lines_still_pass() {
        assert_eq!(
            names("cd crates/x && cargo test 2>&1 | tail -20"),
            ["cd", "cargo", "tail"]
        );
        assert_eq!(names("{ a; b; }"), ["a", "b"]);
        assert_eq!(names("git log --format='%H %s' | head -5"), ["git", "head"]);
    }
}
