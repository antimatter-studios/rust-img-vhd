//! The debug run that lets the PR gate see an overflow guards itself.
//!
//! `overflow-checks` is on in debug and off in release, so a defect
//! whose only symptom is an arithmetic overflow panic cannot be
//! observed by a release-only test run. Measured on this crate rather
//! than inferred from `[profile.dev]` carrying no key: a temporary
//! `black_box(250u8) + black_box(10u8)` panicked under `cargo test
//! --locked` and passed under `cargo test --locked --release`.
//!
//! So the debug run in `ci.yml` -- the workflow that actually gates a
//! merge -- is what makes every arithmetic test in this crate mean
//! anything, and this file is what keeps it there. Nothing else in the
//! repository would notice if that run were deleted, or if `--release`
//! were added to it.
//!
//! THE GUARD IS NOT PROTECTION AGAINST MALICE. It is protection
//! against a tidy-up. Where a repository runs the suite twice, the
//! debug job looks like a duplicate of the release one beside it, and
//! that is exactly why someone removes it. Where it runs once, the
//! plausible edit is the opposite -- adding `--release` for
//! consistency with a sibling or to cut CI time -- which turns the
//! checks off with nothing failing. Both edits are silent; this file
//! is what makes them loud.
//!
//! # Two halves, neither redundant
//!
//! | half | asks | cannot answer |
//! |---|---|---|
//! | the scans here | is the step still in `ci.yml`, asked to check, and not disabled from the manifest | whether the build it produces actually traps |
//! | `overflow_checks` in `src/lib.rs` | does this build trap a real `u64::MAX + 1` | whether it was supposed to; it cannot notice its own absence |
//!
//! Delete the step and the runtime probe never runs at all. Keep the
//! step but drop the variable and the probe runs, finds nothing to
//! check, and passes doing nothing. Keep both and put
//! `overflow-checks = false` under `[profile.test]` and the step is
//! present, running, green and blind. Each needs its own guard.
//!
//! # Why this is an integration test and not a module under `src/`
//!
//! Cargo discovers `tests/*.rs` on its own, so there is no declaration
//! anywhere that can be deleted to switch this off, and `Cargo.toml`
//! sets no `autotests = false`. A guard living as a file under `src/`
//! behind a `#[cfg(test)] mod` line has no such protection: lose the
//! one line and the file stays, compiles into nothing, and asserts
//! nothing, with no lint to say so. That happened once already on a
//! sibling repository's version of this fix -- a `git reset --hard`
//! took the `mod` line, the suite went green, and seven assertions
//! silently ceased to exist.
//!
//! The runtime probe in `src/lib.rs` is the deliberate exception, and
//! is inline in `lib.rs` for the same reason: it must be part of the
//! library target the debug step builds, and inline there is no
//! declaration to lose.

use saphyr::{LoadableYamlNode, Yaml};
use std::path::{Path, PathBuf};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn ci_yml() -> PathBuf {
    manifest_dir()
        .join(".github")
        .join("workflows")
        .join("ci.yml")
}

/// Read a file the guards depend on, or fail.
///
/// It panics rather than returning `None` on purpose. An
/// `if !path.exists() { return }` anywhere in this module would
/// reproduce the exact class of blindness the module exists to prevent:
/// an assertion that is present, runs, and cannot report the thing it
/// was written for. A missing workflow is a finding, not a skip.
fn read_or_panic(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}. This guard must fail rather than skip: a \
             version of it that returned early here would be the same \
             blindness it exists to prevent.",
            path.display()
        )
    })
}

/// The command lines of a shell script, with comments removed.
///
/// ONE PLACE, because two callers used to disagree about what a
/// comment is. `runs_with_overflow_checks` stripped them and
/// `step_declares_the_handshake` read `step.run` raw, so a step could
/// be armed by a line that never executes:
///
///     run: |
///       # EXPECT_OVERFLOW_CHECKS=1 -- see ci_profile.rs
///       cargo test --locked --all-targets
///
/// The handshake half said yes on the comment, the command half found
/// the real run, both workflow assertions passed, and the process got
/// no `EXPECT_OVERFLOW_CHECKS` -- so the runtime probe returned without
/// asserting anything. Two readers of one text must not have two
/// grammars.
///
/// It is shell, not YAML: [`parse_workflow`] has already removed the
/// workflow's own comments, and what reaches here is the inside of a
/// `run:` block, where `#` is the shell's comment character.
fn command_lines(script: &str) -> Vec<&str> {
    script
        .lines()
        .map(str::trim_start)
        .map(|line| match comment_start(line) {
            Some(at) => &line[..at],
            None => line,
        })
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

/// Where a shell comment begins on a line, if it does.
///
/// A `#` is the comment character only where a WORD begins: at the
/// start of the line, or after a character that ends a word. The rule
/// used to be `line.split(" #")`, which is that rule written for
/// exactly one such character, so a comment glued to a terminator
/// survived into the line and was read as part of it:
///
///     cargo test --locked --lib;# EXPECT_OVERFLOW_CHECKS=1 is set in CI
///
/// The handshake half found the variable in the comment, the command
/// half found a real debug run before it, and the step counted as
/// arming a probe the process would never receive.
///
/// THE ALPHABET IS [`shell_commands`]' OWN SEPARATOR SET, not a second
/// one invented here. That is the whole point: two readers of one text
/// must not have two grammars, which is why `command_lines` exists at
/// all. `|` and a tab are in the set and were not holes -- other
/// mechanisms already caught them -- and they are handled here because
/// the rule is "a word begins", not because either was measured
/// escaping.
///
/// A `#` inside quotes is data, and so is one inside a word:
/// `--features a#b` is one argument, and cutting at it would truncate
/// a real command and make the guard refuse a correct workflow.
fn comment_start(line: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut at_word_start = true;
    for (index, c) in line.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            at_word_start = false;
            continue;
        }
        match c {
            '#' if at_word_start => return Some(index),
            '\'' | '"' => {
                quote = Some(c);
                at_word_start = false;
            }
            ';' | '&' | '|' | '(' | ')' => at_word_start = true,
            c if c.is_whitespace() => at_word_start = true,
            _ => at_word_start = false,
        }
    }
    None
}

/// Whether a line turns `set -e` off.
///
/// Actions runs a `run:` block as `bash -e`, so a failing command ends
/// the step. `set +e` withdraws that for everything after it, which
/// makes every command in the block advisory -- including the one this
/// file exists to require. Recognised in all its spellings (`set +e`,
/// `set +ex`, `set +o errexit`) rather than as a fixed string.
///
/// IT TOKENISES WITH [`shell_commands`] RATHER THAN `split_whitespace`.
/// Whitespace is not what ends a word in a shell, so the option name
/// with the next command's punctuation glued to it -- `set +o
/// errexit;`, `set +o errexit&&` -- read as `errexit;`, which is not
/// `errexit`, and the withdrawal was invisible. Only the detached
/// `set +o errexit ;` was caught, which is the spelling nobody writes.
///
/// Sharing the tokeniser also settles the quoted case for free:
/// `echo "set +o errexit"` yields the words `["echo", ""]`, so a
/// printed withdrawal withdraws nothing.
fn disables_errexit(line: &str) -> bool {
    shell_commands(line)
        .iter()
        .any(|(words, _)| set_disables_errexit(words))
}

/// Whether one tokenised command is a `set` that turns `errexit` off.
fn set_disables_errexit(words: &[String]) -> bool {
    let mut words = words.iter().map(String::as_str);
    if words.next() != Some("set") {
        return false;
    }
    let mut expecting_option_name = false;
    for word in words {
        if expecting_option_name {
            if word == "errexit" {
                return true;
            }
            expecting_option_name = false;
            continue;
        }
        if word == "+o" {
            expecting_option_name = true;
            continue;
        }
        if let Some(flags) = word.strip_prefix('+') {
            if flags.contains('e') {
                return true;
            }
        }
    }
    false
}

/// What follows a command on its line, and therefore whether the shell
/// reads the command's exit status.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sep {
    /// Nothing -- the command ends the line.
    End,
    /// `&&`: the chain fails if this command fails.
    And,
    /// `||`: the failure is caught and discarded.
    Or,
    /// `;`: under `bash -e` a failure still ends the step, because
    /// `-e` aborts before the next command runs. MEASURED, not
    /// reasoned: `bash -e -c 'false; echo REACHED'` prints nothing and
    /// exits 1.
    Semi,
    /// `|`: the line's status becomes the LAST command's. Actions'
    /// default `bash -e` does not set `pipefail`.
    Pipe,
    /// `&`: backgrounded, so nothing waits for it.
    Amp,
}

/// Index of the `)` that closes the `$( )` / `<( )` / `>( )` span whose
/// `(` is at `open`, or `None` if the span is never closed.
///
/// # WHY THIS IS NOT A PAREN COUNT
///
/// It was one, and it was wrong in BOTH directions, because deciding
/// where a span ends is itself shell parsing. Verdicts below taken from
/// `bash -e -c`, not from reading this file:
///
/// | line | bash | a bare paren count |
/// |---|---|---|
/// | `cat <(echo a\) ; false)` | exit 0 — swallowed | closes at `\)`, so the `false` reads as top-level: **a swallowed suite counted as a gate** |
/// | `cat <(echo "a)") ; false` | exit 1 — the `false` IS read | closes at the quoted `)`, then the stray `"` opens a quote that eats the rest: **a real gate refused** |
///
/// The first is the defect this file exists to prevent. The second is
/// the one this file's own history warns about — a guard that starts
/// refusing correct workflows — and it is why a `\)` special case would
/// not have been a fix.
///
/// The rules are the shell's, and the same ones the tokeniser below
/// already applies fifteen lines on: outside quotes a backslash escapes
/// the next character; inside `"` it still does; inside `'` there are no
/// escapes at all and a backslash is data. Parentheses inside any quote
/// are data.
fn end_of_substitution(chars: &[char], open: usize) -> Option<usize> {
    debug_assert_eq!(chars.get(open), Some(&'('), "open must index the `(`");
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut j = open;
    while j < chars.len() {
        let c = chars[j];
        match quote {
            // No escapes inside `'`; only another `'` ends it.
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                }
            }
            Some(q) => {
                if c == '\\' && j + 1 < chars.len() {
                    j += 2;
                    continue;
                }
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == '\\' && j + 1 < chars.len() {
                    j += 2;
                    continue;
                }
                // A NESTED SPAN'S PARENTHESES ARE ITS OWN. Quotes and
                // escapes were tracked and these two were not, so a `)`
                // inside a nested backtick span or a `${...}` closed the
                // OUTER substitution early. Both directions were
                // measured against `bash -e -c`:
                //
                //   echo $(echo `echo x)`) ; cargo test --lib
                //       bash reads the trailing status; the span closed
                //       at the `)` inside the backticks, the unmatched
                //       backtick then ate the rest of the line, and a
                //       real gate was DROPPED.
                //
                //   $(echo ${x:+)} ; cargo test --lib)
                //       bash swallows it; the span closed at the `)`
                //       inside `${...}`, so the `cargo test` read as
                //       top-level and a swallowed suite was COUNTED.
                //
                // Skipping the nested span is the same answer as for a
                // quote: what is inside it is not this scanner's
                // punctuation.
                if c == '`' {
                    match end_of_backticks(chars, j) {
                        Some(close) => {
                            j = close + 1;
                            continue;
                        }
                        None => return None,
                    }
                }
                if c == '$' && j + 1 < chars.len() && chars[j + 1] == '{' {
                    match end_of_braces(chars, j + 1) {
                        Some(close) => {
                            j = close + 1;
                            continue;
                        }
                        None => return None,
                    }
                }
                match c {
                    '\'' | '"' => quote = Some(c),
                    '(' => depth += 1,
                    ')' => {
                        // `depth` is at least 1 here because `open`
                        // indexes a `(`; subtracting without checking
                        // would panic in the debug profile the PR gate
                        // runs precisely so an overflow is visible.
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            return Some(j);
                        }
                    }
                    _ => {}
                }
            }
        }
        j += 1;
    }
    None
}

/// Index of the `}` closing the `${...}` opened at `open`, which
/// indexes the `{`, or `None` if it is never closed.
///
/// Brace expansions nest — `${x:-${y}}` is legal — so this counts
/// braces the way [`end_of_substitution`] counts parentheses, and for
/// the same reason: a `}` belonging to an inner expansion is not the
/// outer one's close.
///
/// Quoting inside `${...}` is NOT tracked, and the limit is stated
/// rather than implied. The expansion's own grammar allows a quoted
/// word in the default value, so `${x:-"}"}` would close early here.
/// No workflow in this constellation writes one, and the effect is to
/// end the outer span early, which drops a gate rather than inventing
/// one — the direction this file declares as safe.
fn end_of_braces(chars: &[char], open: usize) -> Option<usize> {
    debug_assert_eq!(chars.get(open), Some(&'{'), "open must index the brace");
    let mut depth = 0usize;
    let mut j = open;
    while j < chars.len() {
        // A NESTED SPAN'S CONTENTS ARE DATA, WHICH IS THE SAME RULE ONE
        // LAYER DOWN. This counted every `{` and `}`, so a literal
        // brace inside a command substitution -- where it is an
        // ordinary argument character -- read as another expansion
        // level, the closing `}` never brought the depth back to zero,
        // this answered `None`, `shell_commands` treated the span as
        // unterminated and dropped the rest of the line, and an `&&`
        // chain that did NOT end the step looked as though it did:
        //
        //   cargo test --locked --lib && echo ${x:-$(echo {)} ; true
        //
        // bash exits 0 whether the suite passes or fails -- the
        // `; true` swallows it -- and the guard counted it as the gate.
        // Measured both ways with `bash -e -c`; and
        // `echo "${x:-$(echo {)}"` prints `{`, so the brace really is
        // data.
        //
        // `braces_no_nesting` passing was never evidence this was
        // right: it shows the depth counter does something, not that it
        // is correct on a brace that is not an expansion.
        if chars[j] == '$' && j + 1 < chars.len() && chars[j + 1] == '(' {
            match end_of_substitution(chars, j + 1) {
                Some(close) => {
                    j = close + 1;
                    continue;
                }
                None => return None,
            }
        }
        if chars[j] == '`' {
            match end_of_backticks(chars, j) {
                Some(close) => {
                    j = close + 1;
                    continue;
                }
                None => return None,
            }
        }
        // ONLY `${` OPENS A LEVEL, apart from the opening brace this
        // was called on. A bare `{` elsewhere is data.
        if chars[j] == '$' && j + 1 < chars.len() && chars[j + 1] == '{' {
            depth += 1;
            j += 2;
            continue;
        }
        match chars[j] {
            '\\' if j + 1 < chars.len() => {
                j += 1;
            }
            '{' if j == open => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// Index of the backtick closing the span opened at `open`, or `None`.
///
/// Same blindness, same direction: inside backticks a `\` escapes the
/// next character, so `` `echo a\` ; false` `` is ONE substitution and
/// its status is swallowed — measured, `exit 0`. Stopping at the escaped
/// backtick resumed tokenising inside the span and read the `false` as a
/// top-level command, counting a suite whose failure nothing sees.
///
/// Quoting is deliberately NOT tracked here. Backtick spans nest quotes
/// and escapes in a way that needs its own grammar, no workflow in this
/// constellation writes one, and an untested branch in a guard is worth
/// less than a stated limit. The escape is handled because it is the
/// spelling that produces a FALSE PASS.
fn end_of_backticks(chars: &[char], open: usize) -> Option<usize> {
    debug_assert_eq!(chars.get(open), Some(&'`'), "open must index the backtick");
    let mut j = open + 1;
    while j < chars.len() {
        if chars[j] == '\\' && j + 1 < chars.len() {
            j += 2;
            continue;
        }
        if chars[j] == '`' {
            return Some(j);
        }
        j += 1;
    }
    None
}

/// One shell line split into the commands it invokes, each with the
/// separator that follows it, and with the contents of quoted spans
/// dropped.
///
/// It is not a shell. It knows four things: the separators above; that
/// whitespace divides words; that what sits inside `'` or `"` is DATA
/// rather than a word; and that an `&` right after `>` or `<` is part
/// of a redirection (`2>&1`) rather than a separator.
///
/// The quoting rule is the whole difference between running a command
/// and printing it:
///
///     cargo test --locked --lib      -> [["cargo", "test", ...]]
///     echo "cargo test --locked"     -> [["echo", ""]]
///
/// The characters are the same and the substring match that used to
/// stand here could not tell them apart.
///
/// OVER-STRICT IS THE SAFE DIRECTION, as everywhere else in this file:
/// a shape it fails to recognise makes the guard refuse a correct
/// workflow, loudly, with the command quoted. The other way round is a
/// gate that went blind and said nothing.
fn shell_commands(line: &str) -> Vec<(Vec<String>, Sep)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<(Vec<String>, Sep)> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut i = 0;

    macro_rules! end_word {
        () => {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        };
    }

    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            // Inside a quoted span every character is data, including a
            // separator: `echo "a && cargo test"` is one command.
            //
            // A BACKSLASH INSIDE `"` ESCAPES THE NEXT CHARACTER, and
            // without this the span ended at the first `\"`:
            // `echo "a \" && cargo test"` split where the shell does
            // not, and the second half read as a command being run. In
            // `'` there are no escapes at all -- a backslash is data --
            // which is why this is conditional on the quote character.
            if q == '"' && c == '\\' && i + 1 < chars.len() {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        // A COMMAND SUBSTITUTION SWALLOWS ITS COMMAND'S STATUS, so what
        // is inside one is data exactly as a quoted span is.
        // `$(` and `)` used to fall into the separator arm below and
        // become `Sep::Semi`, so `echo $(cargo test --locked --lib)`
        // parsed as three commands with the inner one looking
        // status-read. Measured: `bash -e -c 'echo $(false); echo R'`
        // exits 0 and prints R -- only `echo`'s status is read.
        //
        // A BARE `( … )` SUBSHELL IS NOT THIS. Its status IS read, so it
        // keeps falling through to the separator arm.
        // `<( … )` AND `>( … )` ARE THE SAME SWALLOWING. A process
        // substitution hands the command a file name to read, and the
        // command's own status is all the line reports. Measured:
        //
        //   bash -e -c 'cat <(false)'          exit 0
        //   bash -e -c 'cat <(false); echo R'  exit 0, R printed
        //
        // Only `$(` was recognised, so `cat <(cargo test --locked
        // --all-targets)` split at the `(` into a `cat <` command and a
        // `cargo test` one that looked status-read -- and the guard
        // counted a suite whose failure nothing could see.
        //
        // FINDING THE END OF THE SPAN IS ITSELF SHELL PARSING, and the
        // first version of this counted bare parentheses. See
        // [`end_of_substitution`]: a `\)` or a `")"` inside the span is
        // data, and taking either for the close resumed tokenising in
        // the middle of a span the shell had not left.
        if matches!(c, '$' | '<' | '>') && i + 1 < chars.len() && chars[i + 1] == '(' {
            started = true;
            i = match end_of_substitution(&chars, i + 1) {
                Some(close) => close + 1,
                // Unterminated: the shell would not accept this line at
                // all, and there is no command after it to read.
                None => chars.len(),
            };
            continue;
        }
        if c == '`' {
            started = true;
            i = match end_of_backticks(&chars, i) {
                Some(close) => close + 1,
                None => chars.len(),
            };
            continue;
        }
        // A `${...}` EXPANSION IS DATA HERE TOO, and this site was
        // missed when the same rule went into `end_of_substitution`: a
        // fix applied one function away and not applied here. `$` and
        // `{` were ordinary word characters, so the `)` in
        //
        //   cargo test --locked --lib && echo ${x:+)}
        //
        // fell into the separator arm below and split the line. The
        // `&&` chain then no longer ENDED the step, which is this
        // file's rule for whether an `&&` list's failure is read, so
        // the suite's failure read as swallowed and a real gate was
        // dropped. Measured: `bash -e -c 'x=1; false && echo ${x:+)}'`
        // exits 1 and the `true` spelling exits 0, so the status is
        // read.
        if c == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
            started = true;
            i = match end_of_braces(&chars, i + 1) {
                Some(close) => close + 1,
                None => chars.len(),
            };
            continue;
        }
        match c {
            '\\' if i + 1 < chars.len() => {
                // Outside quotes a backslash escapes the next character,
                // so `echo a\&\& b` is one command, not two.
                word.push(chars[i + 1]);
                started = true;
                i += 2;
            }
            '\'' | '"' => {
                quote = Some(c);
                started = true;
                i += 1;
            }
            '&' | '|' | ';' | '(' | ')' => {
                // `2>&1` and `>&2`: an `&` bound to a redirection is
                // part of the word, and the command's status is still
                // read.
                if c == '&' && started && (word.ends_with('>') || word.ends_with('<')) {
                    word.push(c);
                    i += 1;
                    continue;
                }
                let doubled = i + 1 < chars.len() && chars[i + 1] == c;
                let sep = match (c, doubled) {
                    ('&', true) => Sep::And,
                    ('&', false) => Sep::Amp,
                    ('|', true) => Sep::Or,
                    ('|', false) => Sep::Pipe,
                    _ => Sep::Semi,
                };
                end_word!();
                if !words.is_empty() {
                    out.push((std::mem::take(&mut words), sep));
                }
                i += if doubled && c != ';' { 2 } else { 1 };
            }
            c if c.is_whitespace() => {
                end_word!();
                i += 1;
            }
            _ => {
                word.push(c);
                started = true;
                i += 1;
            }
        }
    }
    if started {
        words.push(word);
    }
    if !words.is_empty() {
        out.push((words, Sep::End));
    }
    out
}

/// The arguments of a `cargo test` invocation on this line, or `None`
/// if the line does not invoke one.
///
/// The scan used to ask `command.contains("cargo test")`, and a line
/// that only PRINTS the command satisfied it:
///
///     echo "EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib"
///
/// That one line met both of this file's workflow assertions -- the
/// debug run and the handshake -- with no debug run behind either. A
/// check whose result does not depend on the thing it exists to detect
/// is this constellation's named defect, found inside a guard written
/// to prevent it.
///
/// Leading `NAME=value` assignments and an `env` prefix are stepped
/// over, because `EXPECT_OVERFLOW_CHECKS=1 cargo test …` is exactly the
/// spelling this guard is looking for; so is a `+toolchain` selector
/// between `cargo` and its subcommand. A wrapper -- `sudo`, `xargs`, a
/// script -- is not recognised and the command does not count, which is
/// the strict direction.
fn cargo_test_arguments(words: &[String]) -> Option<Vec<&str>> {
    let mut words = words
        .iter()
        .map(String::as_str)
        .skip_while(|w| *w == "env" || (!w.starts_with('-') && w.contains('=')));
    let program = words.next()?;
    if program != "cargo" && !program.ends_with("/cargo") {
        return None;
    }
    let mut rest = words.skip_while(|w| w.starts_with('+'));
    if rest.next()? != "test" {
        return None;
    }
    // A REDIRECTION IS NOT AN ARGUMENT. `2>&1` would otherwise read as
    // a bare test-name filter and disqualify a run that is fine, and a
    // separate `>` takes the following word with it.
    let mut arguments = Vec::new();
    let mut argument_is_a_redirection_target = false;
    for word in rest {
        if argument_is_a_redirection_target {
            argument_is_a_redirection_target = false;
            continue;
        }
        if word.contains('>') || word.contains('<') {
            argument_is_a_redirection_target = word.ends_with('>') || word.ends_with('<');
            continue;
        }
        arguments.push(word);
    }
    Some(arguments)
}

/// Cargo options that take their value as the NEXT argument.
///
/// Needed only to tell an option's value from a bare test-name filter:
/// `--features qemu-validation` is not a filter and
/// `cargo test --locked qemu` is.
const OPTIONS_TAKING_A_VALUE: [&str; 18] = [
    "-p",
    "--package",
    "--exclude",
    "-F",
    "--features",
    "--target",
    "--target-dir",
    "--manifest-path",
    "--profile",
    "--test",
    "--bin",
    "--example",
    "--bench",
    "-j",
    "--jobs",
    "--message-format",
    "--color",
    "--config",
];

/// Whether these arguments select something OTHER than the crate's
/// library unit tests, where the overflow probe lives.
///
/// `--test <name>` builds one integration target and no library unit
/// tests. Several repositories here run a cross-validation suite that
/// way, in its own job, beside the real one.
///
/// This used to be `command.contains("--test ")`, which is one of that
/// option's two spellings, and which said nothing at all about `--doc`,
/// `--no-run`, `--bins`, or a bare filter. So four different one-line
/// edits each left the guard green with the probe unbuilt or unrun --
/// `--no-run` most starkly, since it compiles and executes nothing.
///
/// THE ANSWER IS TO COMPARE THE ARGUMENTS, NOT TO WIDEN THE SUBSTRING.
/// The trailing space in the old match was doing real work: `--tests`
/// DOES build the library unit tests and contains `--test`, so dropping
/// the space would have excluded a run that genuinely satisfies this
/// guard. Whole arguments answer both spellings and the `--tests` near
/// miss at once, with no space left load-bearing.
///
/// Everything after a bare `--` belongs to the test harness, not to
/// cargo -- `-- --test-threads=1` selects no target -- so the walk
/// stops there. A filter passed to the harness that way would still
/// narrow the run; that is not covered, and the comment says so rather
/// than the code implying otherwise.
fn omits_the_library_unit_tests(arguments: &[&str]) -> bool {
    let cargo_arguments = arguments.iter().take_while(|a| **a != "--");
    let mut expecting_a_value = false;
    for argument in cargo_arguments {
        if expecting_a_value {
            expecting_a_value = false;
            continue;
        }
        if OPTIONS_TAKING_A_VALUE.contains(argument) {
            // `--test` and friends disqualify whether or not the name
            // is attached, so answer before consuming the value.
            expecting_a_value = true;
        }
        let selects_elsewhere = [
            "--test",
            "--doc",
            "--no-run",
            "--bin",
            "--example",
            "--bench",
        ]
        .iter()
        .any(|o| *argument == *o || argument.starts_with(&format!("{o}=")))
            || ["--bins", "--examples", "--benches"].contains(argument);
        if selects_elsewhere {
            return true;
        }
        // A bare word is a test-name filter, which runs only the tests
        // matching it -- the probe among the ones it may exclude.
        if !argument.starts_with('-') {
            return true;
        }
    }
    false
}

/// Whether the shell reads this command's exit status.
///
/// Nothing here used to look at status handling at all, so
/// `cargo test --locked --all-targets || true` was matched, counted as
/// gating, and gated nothing: the job goes green with the probe
/// failing. A pipe and a trailing `&` are the same edit in other
/// spellings.
///
/// This file already enumerates two levels of the same defect -- a
/// step's `if:` and `continue-on-error:`, and a job's. Suppression
/// inside the command is the third, and it was not on the list.
///
/// WHICH SEPARATORS DISCARD A STATUS IS MEASURED, NOT REASONED.
/// Actions runs a `run:` block as `bash -e` with no `pipefail`, and
/// under that shell:
///
/// ```text
/// bash -e -c 'false; echo REACHED'  prints nothing, exit 1  READ
/// bash -e -c 'false && echo x'                     exit 1   READ
/// bash -e -c 'false || true'                       exit 0   discarded
/// bash -e -c 'false | cat'                         exit 0   discarded
/// bash -e -c 'false &'                             exit 0   discarded
/// bash -e -c 'set +e; false; echo REACHED'  prints, exit 0  discarded
/// ```
///
/// So `;` belongs with `&&`. The first version of this rule refused
/// it, reasoning that the line's status becomes the next command's --
/// true without `-e`, false with it, and the sort of claim that has to
/// be run rather than thought about.
///
/// `set +e` is the caller's to handle, because it disqualifies the
/// whole block rather than one line.
///
/// It stays STRICTER than bash in two places, and both are stated
/// rather than accidental:
///
/// - `cargo test … & wait $!` does propagate the failure (measured:
///   exit 1) and is refused anyway, because recognising it means
///   tracking which job `$!` names;
/// - `cargo test --locked --all-targets && echo ok | tee log` is
///   refused because the tail contains a `Pipe`, and bash reads that
///   status: `|` binds tighter than `&&`, so the pipeline is the last
///   member of the list rather than something after it. MEASURED:
///   `bash -e -c 'false && echo ok | tee /dev/null'` exits 1.
///   `grep -cE 'cargo test.*&&.*\|'` is 0 in all six copies' `ci.yml`,
///   so nothing hits it today; fixing it means telling a pipeline
///   INSIDE a list from one that follows it, which is a change to the
///   tokenizer rather than to this rule.
///
/// Refusing a correct workflow loudly is this file's declared
/// direction; passing a broken one silently is the defect it exists
/// for.
fn status_is_read(
    commands: &[(Vec<String>, Sep)],
    index: usize,
    is_last_command_line: bool,
) -> bool {
    let tail = &commands[index..];
    if !tail
        .iter()
        .all(|(_, sep)| matches!(sep, Sep::End | Sep::And | Sep::Semi))
    {
        return false;
    }
    // AN `&&` LIST ONLY CARRIES ITS FAILURE IF IT ENDS THE LINE, and
    // this cost a measurement to get right. `set -e` exempts a command
    // that is part of an `&&` list from aborting the shell -- and the
    // list's own non-zero status does not abort it either, so whatever
    // follows the list runs and its status becomes the line's:
    //
    //   bash -e -c 'false && echo x'                exit 1   READ
    //   bash -e -c 'false && echo x; echo REACHED'  exit 0   DISCARDED
    //   bash -e -c 'false && echo x && echo y'      exit 1   READ
    //
    // So `cargo test --locked --lib && echo ok; echo done` is a gate
    // that cannot fail, and the rule above -- which asked only that no
    // `||`, `|` or `&` follow -- counted it as read.
    //
    // A NEWLINE ENDS A COMMAND THE SAME WAY `;` DOES, and this rule
    // could not see one. `runs_with_overflow_checks` calls this per
    // line, so "ends the line" was the whole test -- and a `run: |`
    // block is the spelling a workflow is actually written in:
    //
    //   run: |
    //     cargo test --locked --all-targets && echo done
    //     echo "second line"
    //
    // The `&&` list ends its own line, the tail is `[And, End]`, and
    // the step exits 0 with the suite red. Measured:
    // `bash -e -c $'false && echo x\necho R'` exits 0 and prints R,
    // exactly as the `;` spelling does.
    //
    // `an_and_list_that_does_not_end_the_line_has_its_failure_swallowed`
    // is named for this defect and reaches only the `;` half: both of
    // its inputs put the following command on the SAME line. So the
    // rule needs to know whether anything runs after this LINE too,
    // which is a fact about the script rather than about the line.
    if tail[0].1 == Sep::And {
        return is_last_command_line
            && tail
                .iter()
                .all(|(_, sep)| matches!(sep, Sep::And | Sep::End));
    }
    true
}

/// Every `cargo test` invocation in a shell script that would be
/// compiled with overflow checks on AND whose failure would be read.
///
/// The argument is the SHELL text of one step's `run:`, not YAML.
/// [`parse_workflow`] has already turned the workflow into a structure,
/// so a YAML comment can no longer reach this function at all -- that
/// half of the old scan is now the parser's job, by construction. Shell
/// comments still reach here, and [`command_lines`] is the one place
/// they are removed.
///
/// Seven things disqualify a command, and each one is a way the guard
/// could otherwise be satisfied by something that does not actually
/// build the library in debug and fail loudly:
///
/// - the script disables `set -e`, which makes every command in it
///   advisory ([`disables_errexit`]);
/// - it is a shell comment, or an inline trailing comment on an
///   otherwise-`--release` line ([`command_lines`]);
/// - it does not invoke `cargo test` at all, only mentions it
///   ([`cargo_test_arguments`]);
/// - it passes `--release`, or names a profile explicitly;
/// - it sets a `CARGO_PROFILE_*` variable, which can turn overflow
///   checks off for the dev or test profile from outside the manifest;
/// - it selects something other than the library unit tests
///   ([`omits_the_library_unit_tests`]);
/// - its exit status is discarded ([`status_is_read`]).
///
/// A `cargo build` is not a `cargo test` and is not considered, nor is
/// any step that invokes no cargo at all.
fn runs_with_overflow_checks(script: &str) -> Vec<String> {
    let lines = command_lines(script);
    if lines.iter().any(|line| disables_errexit(line)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let last_line = lines.len().saturating_sub(1);
    for (line_index, line) in lines.iter().enumerate() {
        if line.contains("--release") || line.contains("--profile") {
            continue;
        }
        if line.contains("CARGO_PROFILE_") {
            continue;
        }
        let commands = shell_commands(line);
        for (index, (words, _)) in commands.iter().enumerate() {
            let Some(arguments) = cargo_test_arguments(words) else {
                continue;
            };
            if omits_the_library_unit_tests(&arguments) {
                continue;
            }
            // Whether an `&&` chain's failure reaches the step depends
            // on nothing running after it -- on a later line included.
            if !status_is_read(&commands, index, line_index == last_line) {
                continue;
            }
            out.push(line.to_string());
            break;
        }
    }
    out
}

/// WHAT ELSE DECIDES WHETHER A STEP GATES.
///
/// The first version of this guard matched the text of a `- run:` line
/// and never looked at anything else in the step. That is enough to
/// find the command and useless for deciding whether the command's
/// result is read. Measured against this repository's own workflow:
/// adding `if: false` to the step, or `continue-on-error: true`, left
/// every one of the guard's 31 tests green while the gate went blind.
/// A step that runs and whose result nothing reads is this
/// constellation's own named defect, reproduced inside the guard
/// written to prevent it.
///
/// So the list is enumerated first, rather than discovered one defeat
/// at a time. A `run:` step gates a pull request only if ALL of these
/// hold:
///
/// 1. the step carries no `if:` -- a false condition skips it;
/// 2. the step carries no `continue-on-error:` -- its failure is
///    discarded;
/// 3. its JOB carries no `if:` -- same reasoning, one level up;
/// 4. its JOB carries no `continue-on-error:`;
/// 5. the workflow's `on:` still includes `pull_request` -- a scan
///    scoped to `ci.yml` assumes `ci.yml` is what runs on a pull
///    request, and that is a fact about the file, not a given.
///
/// OVER-STRICT IS THE SAFE DIRECTION HERE, so 1 and 2 reject on the
/// key's PRESENCE rather than trying to evaluate it. `if: false`,
/// `if: ${{ false }}`, and an `if:` on an expression that happens to
/// evaluate false are distinct spellings, and this crate has already
/// been caught by four spellings of one manifest key -- enumerating
/// them is the losing game. A step that genuinely needs a condition
/// can be split out; a guard that tries to interpret conditions is a
/// guard with a new defeat every time GitHub adds syntax.
///
/// # Why this is parsed and no longer scanned
///
/// The version this replaces hand-rolled the YAML: `.lines()`, an
/// indent count, `split_once(':')` for the key, and `after != "|"` for
/// a block scalar. It was defeated three more times after the five
/// spellings above, and each defeat was the same shape -- ordinary
/// YAML the scanner had not been taught:
///
/// ```text
///   "if": false               quoted key -- matched no NON_GATING_KEYS
///                             entry, so the step counted as gating
///                             while Actions skipped it. SILENT.
///   "continue-on-error": true same.
///   # pull_request:           a substring match over the `on:` block's
///                             raw text, comments included, so
///                             commenting the trigger out left the
///                             guard green. SILENT.
///   run: |-  / run: >         only a bare `|` opened a block, so every
///                             other legal style was read as the
///                             command itself and the block's contents
///                             never parsed. LOUD -- it failed a
///                             correct workflow.
/// ```
///
/// Quoted keys, block scalar styles, comments and nested mappings are
/// not edge cases; they are the grammar. A parser handles all of them
/// by construction, and does not need to be taught the next one. The
/// sibling `rust-fs-xfs` copy patched each hole individually and its
/// own comments record the cost: the identical quote-normalisation was
/// added to its TOML key scan, and then had to be added again, a few
/// dozen lines away, to its YAML key scan. The same lesson twice in one
/// file is the argument against learning it a third time.
///
/// `saphyr` is a dev-dependency, so nothing here reaches a consumer of
/// the crate.
#[derive(Debug)]
struct Step {
    keys: Vec<String>,
    run: String,
    /// The step's `env:` mapping, as `KEY=VALUE`.
    ///
    /// The handshake is an environment variable, and an inline
    /// `VAR=1 cargo test` prefix is bash syntax. A matrix that includes
    /// `windows-latest` runs the same step under PowerShell, where that
    /// prefix is a syntax error -- so on a cross-platform crate the
    /// handshake HAS to be declared here rather than in the command,
    /// and a guard that only reads the command would refuse the only
    /// spelling that works.
    env: Vec<String>,
}

#[derive(Debug)]
struct Job {
    keys: Vec<String>,
    steps: Vec<Step>,
}

#[derive(Debug)]
struct Workflow {
    triggers: Vec<String>,
    jobs: Vec<Job>,
}

/// The value of `name` in a YAML mapping, or `None`.
///
/// By name rather than by constructing a key, because `saphyr`'s `Yaml`
/// borrows the source text and building one to hand to `get` is more
/// ceremony than the lookup is worth here.
fn field<'a, 'b>(node: &'a Yaml<'b>, name: &str) -> Option<&'a Yaml<'b>> {
    node.as_mapping()?
        .iter()
        .find(|(key, _)| key.as_str() == Some(name))
        .map(|(_, value)| value)
}

/// An env value as text, whether it was written `1`, `"1"` or `true`.
///
/// `EXPECT_OVERFLOW_CHECKS: 1` and `EXPECT_OVERFLOW_CHECKS: "1"` are
/// the same variable to Actions, and a guard that accepted only the
/// quoted spelling would be back to matching spellings.
fn scalar_text(node: &Yaml) -> Option<String> {
    if let Some(text) = node.as_str() {
        return Some(text.to_string());
    }
    if let Some(i) = node.as_integer() {
        return Some(i.to_string());
    }
    node.as_bool().map(|b| b.to_string())
}

/// The keys of a YAML mapping, as plain strings.
///
/// The parser has already resolved the quoting, so `"if"`, `'if'` and
/// `if` all arrive here as `if`. That is the whole of the quoted-key
/// fix: there is no un-quoting step to forget.
fn keys_of(node: &Yaml) -> Vec<String> {
    node.as_mapping()
        .map(|mapping| {
            mapping
                .iter()
                .filter_map(|(key, _)| key.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Structure a workflow far enough to answer the five questions above.
///
/// Panics on a workflow it cannot parse, deliberately. A guard that
/// returned an empty `Workflow` for a file it did not understand would
/// report "no debug run gates this" -- which is a failure, so that
/// direction is safe -- but a guard that returned early with a PASS
/// would be the blindness this module exists to prevent. Failing on the
/// parse error names the real problem instead of a consequence of it.
fn parse_workflow(text: &str) -> Workflow {
    let documents = Yaml::load_from_str(text).unwrap_or_else(|e| {
        panic!(
            "workflow is not valid YAML: {e}. This guard reads the workflow \
             rather than scanning its text, so a file it cannot parse is a \
             failure and never a pass."
        )
    });
    let Some(document) = documents.first() else {
        return Workflow {
            triggers: Vec::new(),
            jobs: Vec::new(),
        };
    };

    // `on:` takes three legal shapes: a mapping of trigger names, a
    // sequence of them, or a single scalar. All three are names.
    //
    // Note that `on` survives as the string key `on` and is not folded
    // into the boolean `true` -- saphyr implements the YAML 1.2 core
    // schema, where only `true`/`false` are booleans. The YAML 1.1
    // reading that would break every GitHub workflow ever written does
    // not apply.
    let triggers = match field(document, "on") {
        Some(on) if on.as_mapping().is_some() => keys_of(on),
        Some(on) if on.as_sequence().is_some() => on
            .as_sequence()
            .into_iter()
            .flatten()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        Some(on) => on.as_str().map(str::to_string).into_iter().collect(),
        None => Vec::new(),
    };

    let mut jobs = Vec::new();
    if let Some(mapping) = field(document, "jobs").and_then(Yaml::as_mapping) {
        for (_, body) in mapping.iter() {
            let steps = field(body, "steps")
                .and_then(Yaml::as_sequence)
                .into_iter()
                .flatten()
                .map(|step| Step {
                    keys: keys_of(step),
                    env: field(step, "env")
                        .and_then(Yaml::as_mapping)
                        .map(|m| {
                            m.iter()
                                .filter_map(|(k, v)| {
                                    Some(format!("{}={}", k.as_str()?, scalar_text(v)?))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    // A `run:` block of any style -- `|`, `|-`, `|+`,
                    // `>`, `>-`, `|2` -- arrives as one string with the
                    // block folded per its own rules, so a command
                    // inside a shell loop is seen whole rather than as
                    // fragments, and no style is mistaken for the
                    // command itself.
                    run: field(step, "run")
                        .and_then(Yaml::as_str)
                        .unwrap_or_default()
                        .to_string(),
                })
                .collect();
            jobs.push(Job {
                keys: keys_of(body),
                steps,
            });
        }
    }

    Workflow { triggers, jobs }
}

/// Does this workflow still run on a pull request at all?
///
/// A whole-name comparison against the parsed trigger keys. The version
/// this replaces asked `wf.triggers.contains("pull_request")` of the
/// `on:` block's raw text -- comments and blank lines included -- so
/// the word appearing anywhere in it satisfied the guard. Commenting
/// the real key out, or deleting it and leaving a comment naming it,
/// left `ci.yml` no longer running on pull requests at all with the
/// guard still green.
///
/// `pull_request_target` DELIBERATELY DOES NOT COUNT, and the omission
/// is the point rather than an oversight. It runs against the base
/// repository with a write token and the repository's secrets, and it
/// checks out the base ref by default -- so a workflow triggered only
/// that way may never build the contributor's code at all, and
/// accepting it as proof the merge is gated is permissive in the worst
/// direction. `rust-fs-xfs#146` and `rust-fs-ext4#149` record it as a
/// live gap in the hand-rolled guard this file replaces, where the
/// clause was written by hand and then copied between repositories.
///
/// A parser has no opinion about `pull_request_target` unless someone
/// writes one. So it is not written. If this repository ever needs it
/// accepted, that is a decision with its own justification, and it
/// comes with a check that the checkout selects the pull request head.
fn runs_on_pull_request(wf: &Workflow) -> bool {
    wf.triggers.iter().any(|t| t == "pull_request")
}

/// Keys whose presence on a step or job means its result does not gate.
const NON_GATING_KEYS: [&str; 2] = ["if", "continue-on-error"];

/// Walk a workflow's steps and collect what `select` finds in each
/// `run:`.
///
/// `gating` restricts the walk to steps whose result the pull-request
/// gate actually reads: the workflow must still trigger on a pull
/// request, and neither the job nor the step may carry a key from
/// [`NON_GATING_KEYS`].
///
/// One walk rather than two. The headline assertion used the line-based
/// scan while only the handshake assertion was step-aware, so under
/// `if: false` the headline PASSED and its failure message would have
/// claimed the pull-request gate could see an overflow when the step it
/// names does not run. Every defeat spelling still turned the suite red
/// through the other assertion, so this was a precision defect rather
/// than a hole -- but it left the "runs without --release" property
/// verified line-based, and defeatable if the handshake assertion were
/// ever weakened. Both halves share this walk now and cannot drift
/// apart again. Found on the sibling `rust-fs-btrfs` copy of this guard
/// and corrected here rather than left to diverge.
fn collect_steps(workflow: &str, gating: bool) -> Vec<Step> {
    let wf = parse_workflow(workflow);
    if gating && !runs_on_pull_request(&wf) {
        return Vec::new();
    }
    let carries_a_non_gating_key =
        |keys: &[String]| keys.iter().any(|k| NON_GATING_KEYS.contains(&k.as_str()));

    let mut out = Vec::new();
    for job in wf.jobs {
        if gating && carries_a_non_gating_key(&job.keys) {
            continue;
        }
        for step in job.steps {
            if gating && carries_a_non_gating_key(&step.keys) {
                continue;
            }
            out.push(step);
        }
    }
    out
}

fn scan_steps(workflow: &str, gating: bool, select: fn(&str) -> Vec<String>) -> Vec<String> {
    collect_steps(workflow, gating)
        .iter()
        .flat_map(|step| select(&step.run))
        .collect()
}

/// Does this step ask the build to prove it traps an overflow?
///
/// Either spelling counts, and both are the same instruction to
/// Actions: the variable inline in the command, or declared in the
/// step's `env:` mapping. The mapping is not a concession -- it is the
/// ONLY spelling that works on a matrix including `windows-latest`,
/// where an inline `VAR=1 cargo test` prefix is a PowerShell syntax
/// error. A guard that read only the command would refuse the correct
/// workflow on every cross-platform crate in this constellation.
///
/// THE INLINE HALF READS COMMANDS, NOT THE RAW BLOCK. It used to be
/// `step.run.contains(...)`, and `step.run` is the block verbatim --
/// comments included -- while the other half of the scan strips them.
/// Two readers of one text with two grammars, so a step could be armed
/// by a line the shell never executes:
///
///     run: |
///       # EXPECT_OVERFLOW_CHECKS=1 -- see ci_profile.rs
///       cargo test --locked --all-targets
///
/// Both workflow assertions passed on that and the process got no
/// variable at all, leaving the runtime probe to return without
/// asserting anything. [`command_lines`] is now the single grammar.
fn step_declares_the_handshake(step: &Step) -> bool {
    command_lines(&step.run)
        .iter()
        .any(|line| line_assigns_the_handshake(line))
        || step
            .env
            .iter()
            .any(|entry| entry == "EXPECT_OVERFLOW_CHECKS=1")
}

/// Whether a shell line ASSIGNS the handshake, rather than mentioning
/// it.
///
/// A `contains` over the line stood here, and `command_lines` had
/// already removed comments -- but a printed mention is not a comment:
///
///     echo "EXPECT_OVERFLOW_CHECKS=1"
///     cargo test --locked --all-targets
///
/// That armed the whole step, `gating_runs_that_prove_the_build_traps`
/// then counted the `cargo test` beneath it as proving the build traps,
/// and the process received no variable at all -- so the runtime probe
/// returned without asserting anything. The same defect as the comment
/// grammar it replaced, one grammar later.
///
/// AN ASSIGNMENT IS A PREFIX OF A COMMAND. Every word before it must be
/// another `NAME=value` or `env`, which is exactly the shape
/// [`cargo_test_arguments`] steps over. `echo EXPECT_OVERFLOW_CHECKS=1`
/// is therefore not one: `echo` is not an assignment.
///
/// `export` is accepted with it, because `export VAR=1` on its own line
/// followed by `cargo test` is a real spelling and refusing it would
/// refuse a correct workflow. `set` is not: `set` does not assign.
/// Whether one tokenised command puts the handshake into an
/// ENVIRONMENT a later process receives.
///
/// # AN ASSIGNMENT ON ITS OWN EXPORTS NOTHING
///
/// This returned true on seeing the handshake anywhere in a command's
/// assignment prefix, and a standalone
///
/// ```text
/// EXPECT_OVERFLOW_CHECKS=1
/// cargo test --locked --lib
/// ```
///
/// is a prefix with no command after it: bash sets a SHELL variable
/// that the `cargo test` on the next line never sees. So the guard
/// reported a probe that could not have run — the recurring defect,
/// inside the guard written to close it. The doc this replaces reasoned
/// about `export VAR=1` and about `echo VAR=1` and was silent on
/// assignment-alone, which is how it survived five reviews.
///
/// Three conditions, and the defect was the absence of the second:
///
/// 1. **Name shape.** [`is_assignment`] requires a name that is a legal
///    shell identifier, so `1abc=x` is not an assignment and neither is
///    a bare `=x`.
/// 2. **Leading position, WITH A COMMAND AFTER IT.** A prefix
///    assignment is exported to the command it prefixes and to nothing
///    else, so with no command there is nothing to export to.
/// 3. **Or an exporting builtin**, which puts it in the shell's own
///    environment and so reaches every later command.
///
/// # THE TEMPTING FIX IS TO REQUIRE `export`, AND IT WOULD BE WRONG
///
/// `EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib` is the spelling
/// this repository's own workflow uses and the one the guard exists to
/// accept. `a_real_assignment_beside_a_run_on_one_line_still_counts`
/// and `every_spelling_that_really_assigns_it_still_arms_the_step` are
/// the acceptance arms, and `require_export` is the mutation that shows
/// what requiring it costs.
///
/// # WHAT IS DELIBERATELY STILL REFUSED
///
/// `declare -x` and `typeset -x` do export — measured in
/// `rust-img-vhdx`'s `assigns_the_handshake`, which is the one copy in
/// the constellation that built that table:
///
/// ```text
/// export X=1      child sees it: 1     declare -x X=1  child sees it: 1
/// declare X=1     child sees it: 0     typeset -x X=1  child sees it: 1
/// typeset X=1     child sees it: 0
/// readonly X=1    child sees it: 0
/// ```
///
/// This file recognises only `export`, so the `-x` spellings are
/// REFUSED. That is the over-strict direction and it is left alone
/// deliberately: recognising them widens what counts as armed, which is
/// the unsafe direction, and no workflow here writes one. The four
/// non-exporting spellings are already refused, because none of them is
/// `export`, `env` or an assignment.
fn line_assigns_the_handshake(line: &str) -> bool {
    shell_commands(line)
        .iter()
        .any(|(words, _)| command_assigns_the_handshake(words))
}

const HANDSHAKE: &str = "EXPECT_OVERFLOW_CHECKS=1";

fn command_assigns_the_handshake(words: &[String]) -> bool {
    // An exporting builtin reaches every later command, so where the
    // assignment sits among its arguments does not matter. `export`
    // with no arguments exports nothing, which `words[1..]` gives for
    // free.
    if words.first().map(String::as_str) == Some("export") {
        return words[1..].iter().any(|word| word == HANDSHAKE);
    }

    // Otherwise it has to be an assignment PREFIX, and a prefix is
    // exported to the command it prefixes.
    let mut saw = false;
    let mut at = 0;
    while at < words.len() {
        let word = words[at].as_str();
        if word == "env" {
            at += 1;
            continue;
        }
        if is_assignment(word) {
            if word == HANDSHAKE {
                saw = true;
            }
            at += 1;
            continue;
        }
        // The program name. Anything matching after it is an argument
        // rather than an assignment.
        break;
    }
    // AND A COMMAND FOLLOWS. `at == words.len()` means the whole
    // command was assignments, which is the defect this function was
    // changed for.
    saw && at < words.len()
}

/// `NAME=value`, the shape a shell reads as an assignment rather than a
/// program name.
fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

/// The run commands of steps that run in debug AND actually gate a
/// pull request -- without requiring the handshake.
fn gating_runs_with_overflow_checks(workflow: &str) -> Vec<String> {
    scan_steps(workflow, true, runs_with_overflow_checks)
}

/// The run commands of steps that both run in debug with the handshake
/// AND actually gate a pull request.
///
/// Step-aware rather than text-aware, because the handshake may be
/// declared in the step's `env:` mapping rather than inline in the
/// command -- see [`step_declares_the_handshake`].
fn gating_runs_that_prove_the_build_traps(workflow: &str) -> Vec<String> {
    collect_steps(workflow, true)
        .into_iter()
        .filter(step_declares_the_handshake)
        .flat_map(|step| runs_with_overflow_checks(&step.run))
        .collect()
}

/// The debug runs that ask the build to prove it traps an overflow.
///
/// A subset of [`runs_with_overflow_checks`]: those which also set the
/// `EXPECT_OVERFLOW_CHECKS` handshake, so that
/// `overflow_checks::the_build_the_gate_asked_to_check_does_check`
/// performs an overflow and fails if the build let it through.
///
/// A run carrying the handshake but also `--release` is not counted,
/// because [`runs_with_overflow_checks`] has already excluded it. Such
/// a step is a misconfiguration and it fails loudly rather than
/// quietly: the checks are legitimately off in release, so the
/// assertion the handshake arms would fire there every time.
/// THE SECOND CALL SITE OF THE SAME QUESTION, and it needs its own
/// witness. `step_declares_the_handshake` asks it of a step and this
/// asks it of one command line; fixing only the first left this one a
/// `contains`, and the suite stayed green because no test drove this
/// path with a printed mention on the same line as a real run:
///
///     echo "EXPECT_OVERFLOW_CHECKS=1" && cargo test --locked --lib
///
/// One line, so `runs_with_overflow_checks` hands the whole thing back
/// as the command, `contains` matched, and the run counted as proving
/// the build traps while the process received no variable.
fn debug_runs_that_prove_the_build_traps(script: &str) -> Vec<String> {
    runs_with_overflow_checks(script)
        .into_iter()
        .filter(|command| line_assigns_the_handshake(command))
        .collect()
}

/// The guard. Reads the workflow this repository's pull requests are
/// gated by and refuses if nothing in it compiles the overflow checks.
///
/// `ci.yml` specifically, not every workflow -- see
/// [`a_checking_debug_run_that_is_not_in_ci_yml_does_not_satisfy_this_guard`],
/// which is the one repository-specific decision in this file.
#[test]
fn the_pr_gate_still_tests_in_a_profile_that_can_see_an_overflow() {
    let path = ci_yml();
    let workflow = read_or_panic(&path);

    let debug_runs = gating_runs_with_overflow_checks(&workflow);
    assert!(
        !debug_runs.is_empty(),
        "no `cargo test` in {} runs without `--release`, so a defect whose \
         only symptom is an arithmetic overflow panic can merge without the \
         PR gate ever seeing it. release.yml already runs a debug suite, and \
         that does not help: it triggers on a version tag, after the change \
         has merged. If the debug step in ci.yml looked redundant beside the \
         release ones, it is not -- see the comment above it.",
        path.display()
    );
}

/// The other half of the workflow scan: the step exists, but does it
/// ask the build anything?
///
/// # Why a handshake rather than more spellings
///
/// The manifest scan below reads `Cargo.toml` and asks whether a known
/// spelling of "overflow checks are off" is present. Several spellings
/// of the key were needed before it was right, and then routes turned
/// up that are not in that file at all: a
/// `CARGO_PROFILE_TEST_OVERFLOW_CHECKS` variable set at step or job
/// level in the workflow, and a `.cargo/config.toml`, which nothing
/// here reads. All of them leave the debug step present, running, green
/// and blind.
///
/// They are all the same shape: a scanner enumerating the ways a thing
/// can be disabled, in the places it happens to look. Another pass buys
/// the next one. So the question is put to the build instead -- perform
/// an overflow, see whether you are stopped -- and this test's job
/// shrinks to making sure the gate still asks it.
#[test]
fn the_debug_run_asks_the_build_to_prove_it_traps_overflows() {
    let path = ci_yml();
    let workflow = read_or_panic(&path);

    let proving = gating_runs_that_prove_the_build_traps(&workflow);
    assert!(
        !proving.is_empty(),
        "no `cargo test` in {} runs without `--release` while setting \
         EXPECT_OVERFLOW_CHECKS=1, so nothing checks whether the profile the \
         gate builds actually traps an arithmetic overflow. Reading \
         Cargo.toml is not enough: the checks can also be turned off by a \
         CARGO_PROFILE_TEST_OVERFLOW_CHECKS variable at step or job level, \
         or by a .cargo/config.toml, neither of which is in any file this \
         test reads. The handshake is what arms the one check that cannot be \
         fooled by where the setting lives.",
        path.display()
    );
}

/// THE DISTINCTION THIS REPOSITORY NEEDS THAT A PORTED COPY WOULD MISS.
///
/// A workflow carrying a checking debug run under a name other than
/// `ci.yml` -- `release.yml`, in this repository's own case -- must not
/// satisfy the guards above. Simulated here with `release.yml`'s actual
/// step shape: a plain `cargo test --locked --all-targets` with no
/// `EXPECT_OVERFLOW_CHECKS`, because that workflow was never asked to
/// carry the handshake and is not asked to.
///
/// The scenario worth pinning is the near miss: even a hypothetical
/// debug run in `release.yml` that DID set the handshake would not make
/// `ci.yml`'s own absence of one acceptable, because `release.yml`
/// triggers too late to gate a merge. Both shapes are asserted below.
///
/// This is why the scan is scoped to one file rather than globbed over
/// `.github/workflows/`. A workflow that triggers on a version tag runs
/// after the change has already merged, so a debug run there does not
/// gate anything; a scan across every workflow would count it and
/// report the gate as sound when no pull request is covered. The
/// guard's correctness therefore comes from WHICH FILE it opens, not
/// from the parser refusing these shapes, and that is a fact worth
/// pinning rather than leaving as a comment someone could stop
/// believing.
#[test]
fn a_checking_debug_run_that_is_not_in_ci_yml_does_not_satisfy_this_guard() {
    let release_yml_as_it_is = "\
jobs:
  test:
    steps:
      - run: cargo test --locked --all-targets
      - run: cargo test --locked --all-targets -- --ignored
";
    assert_eq!(
        scan_steps(release_yml_as_it_is, false, runs_with_overflow_checks),
        vec![
            "cargo test --locked --all-targets".to_string(),
            "cargo test --locked --all-targets -- --ignored".to_string(),
        ],
        "release.yml's real steps ARE debug runs -- the parser counts them, \
         and the only reason they do not satisfy the guard is that the guard \
         never opens that file"
    );
    assert!(
        scan_steps(
            release_yml_as_it_is,
            false,
            debug_runs_that_prove_the_build_traps
        )
        .is_empty(),
        "release.yml carries no handshake, and is not asked to"
    );

    let release_yml_with_a_handshake = "\
jobs:
  test:
    steps:
      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --all-targets
";
    assert_eq!(
        scan_steps(
            release_yml_with_a_handshake,
            false,
            debug_runs_that_prove_the_build_traps
        ),
        vec!["EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --all-targets".to_string()],
        "the parser itself would count this step too -- so widening the scan \
         to every workflow would silently stop catching this repository's \
         actual defect"
    );

    // And the real guards must be reading ci.yml, not one of these.
    let scanned = read_or_panic(&ci_yml());
    assert!(
        !gating_runs_that_prove_the_build_traps(&scanned).is_empty(),
        "the guards above must be satisfied by ci.yml's own content, not by \
         any of the strings in this test"
    );
}

/// The full dotted paths that switch overflow checks off for the
/// profile `cargo test` builds.
///
/// # This compares a whole path, because a key is not a word
///
/// The first version of this scan tracked the `[section]` and compared
/// the key to the literal `"overflow-checks"`. That reads correctly and
/// is defeated by ordinary TOML, because the same setting has several
/// spellings and cargo honours all of them without a warning. Measured
/// on a sibling repository with a runtime `u64::MAX + 1` unit test as
/// the probe -- `cargo test --locked --lib` EXIT=101 means the checks
/// are on, EXIT=0 means they are off, and `cargo metadata --no-deps`
/// was EXIT=0 for every one:
///
/// ```text
///   (nothing)                                          EXIT=101  on
///   [profile.test]  overflow-checks = false            EXIT=0    off
///   [profile.test]  "overflow-checks" = false          EXIT=0    off
///   [profile.test]  'overflow-checks' = false          EXIT=0    off
///   [profile]       test.overflow-checks = false       EXIT=0    off
/// ```
///
/// A bare key, a basic string, a literal string, and a dotted key that
/// puts the profile name on the key side where a section-matching scan
/// never looks. Four of those five defeated the first version, and each
/// leaves the debug step in `ci.yml` present, running, green and blind
/// -- the exact state the guard exists to refuse.
///
/// So the section and the key are joined into one path and normalised
/// per segment, and the comparison is against the whole thing. That
/// covers the spellings above, a quoted *section* (`["profile"."test"]`),
/// and a fully top-level dotted key with no section at all.
///
/// Only `profile.dev` and `profile.test` count. `cargo test` builds the
/// `test` profile, which inherits from `dev`, so either can disable the
/// checks in one line. `profile.release` is deliberately absent: the
/// checks are off there by default, that is what ships, and the release
/// steps exist to test what ships.
fn profiles_disabling_overflow_checks(manifest: &str) -> Vec<String> {
    /// Split a dotted TOML path and strip each segment's quoting, so
    /// that `"profile" . 'test'` and `profile.test` are one path.
    fn normalise(path: &str) -> String {
        path.split('.')
            .map(|segment| {
                segment
                    .trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .trim()
            })
            .collect::<Vec<_>>()
            .join(".")
    }

    const DISABLED: [&str; 2] = [
        "profile.dev.overflow-checks",
        "profile.test.overflow-checks",
    ];

    let mut section = String::new();
    let mut found = Vec::new();
    for raw in manifest.lines() {
        let line = raw.split('#').next().unwrap_or(raw).trim();
        if line.starts_with('[') {
            section = normalise(line.trim_matches(|c| c == '[' || c == ']'));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if value.trim() != "false" {
            continue;
        }
        let key = normalise(key);
        let path = if section.is_empty() {
            key
        } else {
            format!("{section}.{key}")
        };
        if DISABLED.contains(&path.as_str()) {
            found.push(path);
        }
    }
    found
}

/// The half of the property the workflow scans cannot see.
///
/// A debug step in `ci.yml` only buys anything while the profile it
/// builds actually checks. One line -- `overflow-checks = false` under
/// `[profile.test]`, or under this repository's existing
/// `[profile.dev]`, a plausible way to make a slow suite faster --
/// would leave that step present, running, green, and no longer able to
/// observe an overflow, with every workflow assertion above still
/// passing. A guard for half a condition is the defect it was written
/// to prevent.
///
/// The runtime probe in `src/lib.rs` would also catch this. This scan
/// is kept as defence in depth: it fails earlier in the gate and names
/// the offending manifest key, which is a better diagnostic than "the
/// build did not trap".
#[test]
fn the_profile_that_cargo_test_builds_still_checks_for_overflow() {
    let path = manifest_dir().join("Cargo.toml");
    let manifest = read_or_panic(&path);

    let disabled = profiles_disabling_overflow_checks(&manifest);
    assert!(
        disabled.is_empty(),
        "{} sets `overflow-checks = false` under {disabled:?}. `cargo test` \
         builds the `test` profile, which inherits from `dev`, so this \
         switches off the check that the debug step in ci.yml exists to run \
         -- leaving that step present, green, and blind. Put it back, or the \
         debug step is costing a compile and buying nothing.",
        path.display()
    );
}

/// The shell scanner is the part of this that can rot, so it is checked
/// against each shape it has to tell apart.
///
/// Its argument is the shell text of one step's `run:`, not YAML. What
/// used to be tested here as YAML -- a debug command quoted in a `#`
/// line of the workflow -- moved to `gating`, because the parser now
/// answers it by construction and this function never sees it.
mod shell_scan {
    use super::runs_with_overflow_checks;

    /// Whether a line's `cargo test` selects something other than the
    /// library unit tests. A line-level wrapper so these tests read as
    /// shell, the way the workflow does.
    fn selects_away_from_the_library(line: &str) -> bool {
        super::shell_commands(line)
            .iter()
            .filter_map(|(words, _)| super::cargo_test_arguments(words))
            .any(|arguments| super::omits_the_library_unit_tests(&arguments))
    }

    /// A LINE THAT PRINTS THE COMMAND IS NOT A RUN.
    ///
    /// The scan asked whether the line CONTAINED "cargo test", so this
    /// one satisfied both of the file's workflow assertions with
    /// nothing behind them: the debug run and, because the same
    /// characters carry `EXPECT_OVERFLOW_CHECKS=1`, the handshake too.
    #[test]
    fn an_echoed_command_is_not_a_run() {
        for line in [
            "echo \"EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\"",
            "echo 'cargo test --locked --all-targets'",
            "echo cargo test --locked --lib",
            "printf '%s\\n' \"cargo test --locked --lib\"",
            "echo \"running: cargo test\" && cargo build --locked",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line),
                Vec::<String>::new(),
                "{line} prints the command; it does not run it"
            );
        }
    }

    /// THE ACCEPTANCE HALF OF THE SAME CHANGE.
    ///
    /// Recognising the invocation rather than the substring must not
    /// cost the spellings a real workflow uses. Each of these DOES
    /// run the suite in debug and each must still be counted --
    /// including the two where `cargo` is not the first word on the
    /// line.
    #[test]
    fn the_spellings_that_do_invoke_cargo_test_still_count() {
        for line in [
            "cargo test --locked --all-targets",
            "EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib",
            "cd .. && cargo test --locked --lib",
            "cargo test --locked --lib 2>&1",
            "cargo test --locked --lib > test.log",
            "cargo +stable test --locked --lib",
            "env EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line).len(),
                1,
                "{line} runs the suite in debug and must be counted"
            );
        }
    }

    /// The trap this repository actually contains, in the form that
    /// still reaches this function. `ci.yml` documents the debug step
    /// by quoting the command, and a `run: |` block can carry the same
    /// habit in shell comments, where the text survives the command's
    /// deletion.
    #[test]
    fn a_debug_run_quoted_in_a_shell_comment_does_not_count() {
        let block = "\
set -euo pipefail
# Measured on this branch:
#     cargo test --locked --release --lib   ->  EXIT=0
#     EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib   ->  EXIT=101
cargo test --locked --release
";
        assert_eq!(
            runs_with_overflow_checks(block),
            Vec::<String>::new(),
            "a debug command quoted inside a comment is documentation, not a run"
        );
    }

    #[test]
    fn a_real_debug_run_counts() {
        let block = "\
cargo test --locked --release
EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib
";
        assert_eq!(
            runs_with_overflow_checks(block),
            vec!["EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib".to_string()],
        );
    }

    /// A command that is `--release` but which carries a trailing
    /// comment mentioning the debug run.
    #[test]
    fn a_trailing_comment_does_not_promote_a_release_run() {
        let inline = "cargo test --locked --release  # not cargo test --lib\n";
        assert_eq!(
            runs_with_overflow_checks(inline),
            Vec::<String>::new(),
            "the command is --release; the comment after it is not a second run"
        );
    }

    /// A `#` THAT DOES NOT BEGIN A WORD IS NOT A COMMENT.
    ///
    /// The acceptance half of widening the comment rule from `" #"` to
    /// the separator alphabet: a `#` inside a word, or inside quotes,
    /// is data. Cutting there would truncate a real command and the
    /// guard would refuse a correct workflow.
    #[test]
    fn a_hash_that_does_not_begin_a_word_is_not_a_comment() {
        for line in [
            "cargo test --locked --features a#b --all-targets",
            "cargo test --locked --all-targets && echo \"done #1\"",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line).len(),
                1,
                "{line:?} has no comment on it: the `#` is inside a word or a quoted span"
            );
        }
    }

    /// THE QUOTE TRACKING IN `comment_start`, WHICH NOTHING ELSE PINS.
    ///
    /// `a_hash_that_does_not_begin_a_word_is_not_a_comment` looks like
    /// it covers this and does not: its quoted `#` sits AFTER the
    /// `cargo test`, so cutting the line there still leaves the
    /// invocation behind and the guard still counts it. The mechanism
    /// survived deletion with every other test green.
    ///
    /// The witnessing input has to put the quoted `#` FIRST, so that
    /// treating it as a comment removes the run:
    ///
    ///     echo "step # 1"; cargo test --locked --all-targets
    ///
    /// Without quote tracking the line is cut to `echo "step`, no
    /// `cargo test` remains, and the guard REFUSES A CORRECT WORKFLOW.
    /// That is the direction this one guards -- a false rejection, not
    /// a false pass.
    ///
    /// It is asserted here rather than as a workflow because YAML ends
    /// a plain scalar at ` #` itself, so the same line written as
    /// `run: echo "step # 1"; ...` is refused before this function ever
    /// sees it -- a different defect, and an easy way to measure the
    /// wrong thing. Only a block scalar reaches here intact, and at
    /// that point the shell text is what is under test.
    #[test]
    fn a_quoted_hash_before_the_run_does_not_cut_the_line_short() {
        for line in [
            "echo \"step # 1\"; cargo test --locked --all-targets",
            "echo 'step # 1' && cargo test --locked --all-targets",
            "printf '%s\\n' \"# not a comment\"; cargo test --locked --lib",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line).len(),
                1,
                "{line:?} runs the suite: the `#` is inside quotes and ends nothing"
            );
        }
    }

    /// The inline-comment strip, which nothing else here pins. A real
    /// debug run whose trailing comment happens to contain `--release`
    /// must still be counted. Without the strip that word disqualifies
    /// the command, and the guard then fails insisting there is no
    /// debug run while one is sitting in front of it.
    #[test]
    fn a_trailing_comment_naming_release_does_not_disqualify_a_debug_run() {
        let line = "cargo test --locked --lib  # deliberately not --release\n";
        assert_eq!(
            runs_with_overflow_checks(line),
            vec!["cargo test --locked --lib".to_string()],
            "the command is a debug run; --release appears only in its comment"
        );
    }

    /// The ways a run can carry no `--release` and still be built
    /// without the checks.
    #[test]
    fn a_profile_named_another_way_does_not_count() {
        let lines = [
            "cargo test --locked --profile release-with-debug --lib",
            "CARGO_PROFILE_TEST_OVERFLOW_CHECKS=false cargo test --locked --lib",
            "CARGO_PROFILE_DEV_OVERFLOW_CHECKS=false cargo test --locked --lib",
        ];
        for line in lines {
            assert_eq!(
                runs_with_overflow_checks(line),
                Vec::<String>::new(),
                "{line} does not compile the overflow checks"
            );
        }
        assert_eq!(
            lines.len(),
            3,
            "the loop above must have examined every shape"
        );
    }

    /// `cargo build` is not `cargo test`. A workflow that builds a
    /// binary and then exercises it with external tooling runs no test
    /// suite, and a scanner that counted `cargo build --release` would
    /// be looking at the wrong steps entirely.
    #[test]
    fn a_cargo_build_step_is_not_a_test_run() {
        let validate_job = "\
cargo build --locked --release
./target/release/some-tool --check /tmp/image
";
        assert_eq!(
            runs_with_overflow_checks(validate_job),
            Vec::<String>::new(),
            "building a binary is not running a test suite"
        );
    }

    /// A COMMAND WHOSE FAILURE IS DISCARDED IS NOT A GATE.
    ///
    /// Nothing here looked at status handling, so a debug run with its
    /// exit status thrown away was matched, counted as gating, and
    /// gated nothing -- the job goes green with the probe failing.
    /// This file already enumerates the same defect at two levels, a
    /// step's `if:`/`continue-on-error:` and a job's; suppression
    /// inside the command is the third.
    ///
    /// The pipe is the one worth reading twice. Actions runs a `run:`
    /// block as `bash -e` with no `pipefail`, so the line's status is
    /// the LAST stage's -- `tee` always succeeds.
    #[test]
    fn a_run_whose_status_is_discarded_does_not_count() {
        for line in [
            "cargo test --locked --all-targets || true",
            "cargo test --locked --all-targets || echo 'ignored'",
            "cargo test --locked --all-targets | tee test.log",
            "cargo test --locked --all-targets &",
            "set +e\ncargo test --locked --all-targets\n",
            "set +o errexit\ncargo test --locked --all-targets\n",
            "set +ex\ncargo test --locked --all-targets\n",
            // The option name with the next command's punctuation
            // glued to it. `split_whitespace` produced `errexit;`,
            // which is not `errexit`, so the withdrawal was invisible
            // and everything after it was counted as gating.
            "set +o errexit; cargo test --locked --all-targets\n",
            "set +o errexit&& cargo test --locked --all-targets\n",
            "set +o errexit\ncargo test --locked --all-targets | tee log\n",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line),
                Vec::<String>::new(),
                "{line:?} runs the suite and throws the answer away"
            );
        }
    }

    /// THE ACCEPTANCE HALF OF THE STATUS RULE.
    ///
    /// `&&` propagates a failure, `set -e` is the default rather than
    /// something to opt into, and a redirection is not a separator --
    /// the `&` in `2>&1` binds to the `>` before it. A status rule that
    /// refused these would refuse most real workflows.
    #[test]
    fn a_run_whose_failure_still_ends_the_step_counts() {
        for line in [
            "cargo test --locked --all-targets",
            "cargo test --locked --all-targets && echo ok",
            "cd .. && cargo test --locked --all-targets && echo ok",
            "cargo test --locked --all-targets 2>&1",
            // `;` is NOT a discard under `bash -e`: the shell aborts
            // before the next command runs. Measured, and the reason
            // the first version of this rule was wrong.
            "cargo test --locked --all-targets; echo done",
            "cargo test --locked --all-targets ; true",
            "set -euo pipefail\ncargo test --locked --all-targets\n",
            // THE ACCEPTANCE HALF OF THE GLUED-PUNCTUATION RULE.
            // `-o errexit` turns the option ON, and a `set +o errexit`
            // that is only PRINTED withdraws nothing -- the tokenizer
            // drops what is inside quotes, so the word is `echo`.
            "set -o errexit; cargo test --locked --all-targets\n",
            "echo \"set +o errexit\"\ncargo test --locked --all-targets\n",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line).len(),
                1,
                "{line:?} fails the step when the suite fails"
            );
        }
    }

    /// SELECTIONS THAT BUILD OR RUN SOMETHING OTHER THAN THE LIBRARY.
    ///
    /// `--doc`, `--no-run` and a bare filter each leave the overflow
    /// probe unbuilt or unrun while the old scan counted the line as
    /// the required debug suite. `--no-run` is the starkest: it
    /// compiles and executes nothing.
    #[test]
    fn a_selection_that_leaves_the_library_unit_tests_out_does_not_count() {
        for line in [
            "cargo test --locked --doc",
            "cargo test --locked --no-run",
            "cargo test --locked --bins",
            "cargo test --locked --examples",
            "cargo test --locked --bin some_tool",
            "cargo test --locked --example inspect",
            "cargo test --locked some_filter",
            "EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --features qemu-validation qemu",
        ] {
            assert!(
                selects_away_from_the_library(line),
                "{line} selects something other than the library unit tests"
            );
            assert_eq!(
                runs_with_overflow_checks(line),
                Vec::<String>::new(),
                "{line} does not build and run the library unit tests"
            );
        }
    }

    /// BOTH SPELLINGS OF `--test` ARE THE SAME OPTION.
    ///
    /// The scan matched the substring `"--test "`, so the `=` form was
    /// invisible: a cross-validation job building one integration
    /// target counted as a full debug run, and the real one could then
    /// be deleted with this guard still green. Nothing noticed because
    /// every such job here is written the long way today; the guard is
    /// what stops the short way from being silently equivalent.
    #[test]
    fn an_equals_spelled_single_target_does_not_count_either() {
        for line in [
            "cargo test --locked --features qemu-validation --test=qemu_validation",
            "EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --test=some_oracle",
            "cargo test --locked --test=\"qemu_validation\"",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line),
                Vec::<String>::new(),
                "{line} builds one integration target and no library unit tests"
            );
            assert!(
                selects_away_from_the_library(line),
                "{line} names a single integration target"
            );
        }
    }

    /// The acceptance half: options that merely START with `--test`
    /// are not the option, and every one of these builds the library
    /// unit tests.
    #[test]
    fn options_that_only_look_like_test_do_not_disqualify_a_run() {
        for line in [
            "cargo test --locked --tests",
            "cargo test --locked --all-targets",
            "cargo test --locked --lib -- --test-threads=1",
        ] {
            assert!(
                !selects_away_from_the_library(line),
                "{line} does not restrict the run to one integration target"
            );
            assert_eq!(
                runs_with_overflow_checks(line).len(),
                1,
                "{line} builds the library unit tests and must be counted"
            );
        }
    }

    /// A single integration target is not the crate's arithmetic.
    /// Several repositories here run a cross-validation suite as
    /// `--test <name>` in its own job, and counting it would let the
    /// real debug run be deleted with the guard still green.
    #[test]
    fn a_single_integration_target_does_not_count() {
        for line in [
            "cargo test --locked --features qemu-validation --test qemu_validation",
            "EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --test some_oracle",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line),
                Vec::<String>::new(),
                "{line} builds one integration target and no library unit tests"
            );
        }
    }

    /// A COMMAND SUBSTITUTION SWALLOWS ITS COMMAND'S STATUS.
    ///
    /// `(` and `)` fell into the separator arm and became `Sep::Semi`,
    /// so `echo $(cargo test --locked --lib)` parsed as three commands
    /// with the inner one looking status-read. Measured:
    /// `bash -e -c 'echo $(false); echo R'` exits 0 and prints R.
    /// WHERE A SUBSTITUTION ENDS IS SHELL PARSING, NOT A PAREN COUNT.
    ///
    /// The scanner counted bare parentheses, so a `)` that the shell
    /// treats as DATA ended the span early and tokenising resumed
    /// inside a span the shell had not left. Every verdict below is
    /// `bash -e -c` on that exact line, taken from bash rather than
    /// from this file; `exit 0` means the status was swallowed, so the
    /// suite is not a gate and must not be counted.
    ///
    /// | line | bash | must |
    /// |---|---|---|
    /// | `cat <(echo a\) ; false)` | 0 | not count |
    /// | `cat <(echo "a)" ; false)` | 0 | not count |
    /// | `cat <(echo 'a\)' ; false)` | 0 | not count |
    /// | `` echo `echo a\` ; false` `` | 0 | not count |
    /// | `cat <(echo "a)") ; false` | 1 | **count** |
    /// | `cat <(echo 'a)') ; false` | 1 | **count** |
    /// | `cat <(echo a\\) ; false` | 1 | **count** |
    /// A NESTED SPAN'S PARENTHESES ARE ITS OWN — BACKTICKS.
    ///
    /// `end_of_substitution` tracked quotes and escapes and not nested
    /// spans, so a `)` inside a nested backtick span closed the outer
    /// `$( )` early; the unmatched backtick then ate the rest of the
    /// line and a real gate was DROPPED. `bash -e -c` on the exact
    /// line, `false` versus `true` for the suite: exits 1 and 0, so the
    /// trailing status is read and this must COUNT.
    #[test]
    fn a_nested_backtick_does_not_close_the_outer_substitution() {
        let line = "echo $(echo `echo x)`) ; cargo test --locked --lib";
        assert_eq!(
            runs_with_overflow_checks(line),
            vec![line.to_string()],
            "{line}: the `)` is inside a nested backtick span, so the substitution runs on \
             and the cargo test after it is top-level"
        );
    }

    /// THE SAME, FOR `${{...}}`, AND IT FAILS THE OTHER WAY.
    ///
    /// A `)` inside a brace expansion closed the outer substitution
    /// early, so a suite that bash keeps INSIDE the substitution read
    /// as top-level and a swallowed run was COUNTED. Measured:
    /// `bash -e -c 'x=1; $(echo ${{x:+)}} ; false)'` and the same with
    /// `true` both exit 0 — the inner status never reaches the line.
    #[test]
    fn a_brace_expansion_does_not_close_the_outer_substitution() {
        assert_eq!(
            runs_with_overflow_checks("$(echo ${x:+)} ; cargo test --locked --lib)"),
            Vec::<String>::new(),
            "the whole thing is inside `$( )`; the `)` belongs to the brace expansion"
        );
        // ACCEPTANCE: the same brace expansion with the suite OUTSIDE
        // the substitution is a real gate and must still count.
        let line = "echo $(echo ${x:+)}) ; cargo test --locked --lib";
        assert_eq!(
            runs_with_overflow_checks(line),
            vec![line.to_string()],
            "{line}: the substitution closes and the cargo test after it is top-level"
        );
        // AND THE MAIN TOKENISER SKIPS IT TOO, which is a second site
        // for the same rule. `$` and `{` were ordinary word characters
        // there, so this `)` became a separator, the `&&` chain stopped
        // ending the step, and the suite's failure read as swallowed.
        let line = "cargo test --locked --lib && echo ${x:+)}";
        assert_eq!(
            runs_with_overflow_checks(line),
            vec![line.to_string()],
            "{line}: the `)` belongs to the expansion, so the && list still ends the step"
        );

        // A LITERAL BRACE IS NOT AN EXPANSION LEVEL. `${x:-$(echo {)}`
        // expands to `{` -- the brace is an ordinary argument
        // character inside a command substitution. Counting it as a
        // level left the span unterminated, `shell_commands` dropped
        // the rest of the line, and the `&&` chain looked as though it
        // ended the step. `bash -e -c 'x=1; false && echo
        // ${x:-$(echo {)} ; true'` exits 0, and so does the `true`
        // spelling: the `; true` swallows the suite's failure, so this
        // must NOT count.
        assert_eq!(
            runs_with_overflow_checks("cargo test --locked --lib && echo ${x:-$(echo {)} ; true"),
            Vec::<String>::new(),
            "the `; true` swallows the failure; the brace inside the substitution is data"
        );
        // ACCEPTANCE: the same expansion with the chain really ending
        // the step is a gate. `false && echo ${x:-$(echo {)}` exits 1
        // and the `true` spelling 0.
        let line = "cargo test --locked --lib && echo ${x:-$(echo {)}";
        assert_eq!(
            runs_with_overflow_checks(line),
            vec![line.to_string()],
            "{line}: the && chain ends the step, so the suite's failure is read"
        );
        // EVERY NESTED SHAPE THAT CARRIES A BRACE, each measured with
        // `bash -e -c` in both spellings and each SWALLOWED by the
        // `; true`, so none may count. Three arms of the fix are held
        // by exactly one of these:
        //
        //   ${x:-{}              a literal `{` directly in the
        //                        expansion; expands to `{`
        //   ${x:-$(echo })}      a `}` inside a nested substitution;
        //                        expands to `}`
        //   ${x:-`echo }`}       a `}` inside nested backticks;
        //                        expands to `}`
        //   ${x:-$(echo ${y})}   an expansion inside a substitution
        //                        inside an expansion; expands to `q`
        for expansion in [
            "${x:-{}",
            "${x:-$(echo })}",
            "${x:-`echo }`}",
            "${x:-$(echo ${y})}",
        ] {
            let line = format!("cargo test --locked --lib && echo {expansion} ; true");
            assert_eq!(
                runs_with_overflow_checks(&line),
                Vec::<String>::new(),
                "{line}: the `; true` swallows the failure, so this is not the gate"
            );
        }

        // AND THE ACCEPTANCE SIDE OF THE SAME NESTING, which is what
        // holds the substitution skip specifically. Ending the span
        // early at the `}` inside `$( )` leaves a stray `)` that reads
        // as a separator, so the `&&` chain stops ending the step and a
        // real gate is dropped. `bash -e -c 'x=; false && echo
        // "${x:-$(echo })}"'` exits 1 and the `true` spelling 0.
        for expansion in ["${x:-$(echo })}", "${x:-$(echo ${y})}"] {
            let line = format!("cargo test --locked --lib && echo {expansion}");
            assert_eq!(
                runs_with_overflow_checks(&line),
                vec![line.clone()],
                "{line}: the && chain ends the step, so the suite's failure is read"
            );
        }

        // And a literal brace in a bare substitution, no expansion at
        // all, which is the same character in the simpler position.
        assert_eq!(
            runs_with_overflow_checks("cargo test --locked --lib && echo $(echo {) ; true"),
            Vec::<String>::new(),
            "a literal brace in a substitution does not change where the line ends"
        );

        // AND THEY NEST. `${x:-${y}z)}` expands to `qz)` in bash, so
        // the `)` is the expansion's, and a scanner that stopped at the
        // INNER `}` would resume at `z)}` and take that `)` for a
        // separator -- which ends the `&&` chain early and drops a real
        // gate. Measured: `bash -e -c 'y=q; false && echo ${x:-${y}z)}'`
        // exits 1 and the `true` spelling exits 0.
        let line = "cargo test --locked --lib && echo ${x:-${y}z)}";
        assert_eq!(
            runs_with_overflow_checks(line),
            vec![line.to_string()],
            "{line}: the nested expansion owns both braces and the `)`, so the && list              still ends the step"
        );
        assert_eq!(
            runs_with_overflow_checks("$(echo ${x:-${y:+)}} ; cargo test --locked --lib)"),
            Vec::<String>::new(),
            "a nested brace expansion's braces are its own inside a substitution too"
        );
    }

    #[test]
    fn a_paren_the_shell_reads_as_data_does_not_end_a_substitution() {
        for line in [
            // THE FILED DEFECT, and it is the false-pass direction: the
            // escaped `)` looked like the close, so the `cargo test`
            // after it parsed as a top-level command whose status is
            // read -- while bash keeps it inside the substitution and
            // throws its status away.
            "cat <(echo a\\) ; cargo test --locked --lib)",
            // The same cause quoted. Correct today only by accident:
            // the stray `\"` happened to swallow the rest of the line.
            "cat <(echo \"a)\" ; cargo test --locked --lib)",
            // Inside `'` a backslash is DATA, so `\\)` does not even
            // escape -- the `)` is quoted and the span runs on.
            "cat <(echo 'a\\)' ; cargo test --locked --lib)",
            // `$( )` is the same span with a different opener.
            "echo $(echo a\\) ; cargo test --locked --lib)",
            "echo $(echo \"a)\" ; cargo test --locked --lib)",
            // And the same `'a\'` shape with the suite INSIDE: bash
            // closes the quote, so the cargo test is still in the
            // substitution. `exit 0`, measured.
            "cat <(echo 'a\\' ; cargo test --locked --lib)",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line),
                Vec::<String>::new(),
                "{line}: bash keeps the cargo test inside the substitution and discards \
                 its status, so counting it would count a probe that does not gate the step"
            );
        }
    }

    /// A BACKSLASH EXTENDS A BACKTICK SPAN TOO, and this one was not in
    /// the report -- the backtick skip had the identical blindness and
    /// the identical false-pass direction. Measured:
    /// `bash -e -c 'echo `echo a\` ; false`'` exits 0.
    #[test]
    fn an_escaped_backtick_does_not_end_a_backtick_substitution() {
        let line = "echo `echo a\\` ; cargo test --locked --lib`";
        assert_eq!(
            runs_with_overflow_checks(line),
            Vec::<String>::new(),
            "{line}: the escaped backtick keeps the cargo test inside the substitution"
        );
    }

    /// THE ACCEPTANCE HALF, AND IT IS THE HALF A `\)` SPECIAL CASE
    /// WOULD HAVE LEFT BROKEN.
    ///
    /// These three end their substitution properly and leave a real
    /// top-level `cargo test` whose failure DOES end the step. A
    /// scanner that mis-closes early makes the guard refuse a correct
    /// workflow, which is the direction this file's own history warns
    /// about and the direction the quoted spelling actually failed in.
    #[test]
    fn a_substitution_that_really_closes_still_leaves_a_gate() {
        for line in [
            // Was refused before this change: the scanner closed at the
            // quoted `)` and the leftover `"` ate the rest of the line.
            "cat <(echo \"a)\") ; cargo test --locked --lib",
            "cat <(echo 'a)') ; cargo test --locked --lib",
            // `\\` is an escaped BACKSLASH, so the `)` after it really
            // is the close. A scanner that skipped one character per
            // backslash instead of two would get this backwards.
            "cat <(echo a\\\\) ; cargo test --locked --lib",
            // INSIDE `'` A BACKSLASH IS DATA, so `'a\'` is the
            // literal `a\` and the quote CLOSES at the second `'`.
            // Treating `'` the way `"` is treated makes the `\'` an
            // escape, the quote never closes, no `)` is ever found, and
            // this real gate is silently dropped. `bash -e -c
            // "cat <(echo 'a\\') ; false"` exits 1 -- it is read.
            "cat <(echo 'a\\') ; cargo test --locked --lib",
            // The plain control, unchanged by any of this.
            "cat <(echo a) ; cargo test --locked --lib",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line),
                vec![line.to_string()],
                "{line}: the substitution closes and the cargo test after it is top-level, \
                 so its failure ends the step and it must still count"
            );
        }
    }

    #[test]
    fn a_cargo_test_inside_a_command_substitution_is_not_a_gate() {
        for line in [
            "echo $(cargo test --locked --lib)",
            "OUT=$(cargo test --locked --lib)",
            "echo `cargo test --locked --lib`",
            "echo \"result: $(cargo test --locked --all-targets)\"",
            // PROCESS SUBSTITUTION IS THE SAME SWALLOWING. `<( … )`
            // hands the command a file to read; only the command's own
            // status reaches the line. Measured, both directions:
            // `bash -e -c 'cat <(false)'` and
            // `bash -e -c 'echo x > >(false)'` each exit 0.
            "cat <(cargo test --locked --all-targets)",
            "diff <(cargo test --locked --lib) expected.txt",
            "echo x > >(cargo test --locked --all-targets)",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line),
                Vec::<String>::new(),
                "{line}: the substitution swallows the status, only the outer command's is read"
            );
        }
    }

    /// THE ACCEPTANCE HALF. A substitution in an ARGUMENT of a real run
    /// must not stop it counting -- that is the ordinary spelling for a
    /// thread count, and refusing it refuses a correct workflow.
    #[test]
    fn a_substitution_in_an_argument_leaves_the_run_counted() {
        assert_eq!(
            runs_with_overflow_checks("cargo test --locked --lib -- --test-threads=$(nproc)").len(),
            1,
            "the run is `cargo test`; the substitution is one of its arguments"
        );
        // A BARE SUBSHELL IS NOT A SUBSTITUTION. Its status is read, so
        // it keeps falling through to the separator arm.
        assert_eq!(
            runs_with_overflow_checks("cargo test --locked --lib").len(),
            1,
        );
    }

    /// A BACKSLASH-ESCAPED QUOTE DOES NOT CLOSE THE SPAN.
    ///
    /// The quoted-span rule was `if c == q { quote = None }` with no
    /// escape handling, so this split where the shell does not and the
    /// tail read as a command being run.
    #[test]
    fn an_escaped_quote_does_not_end_the_quoted_span() {
        for line in [
            "echo \"a \\\" && cargo test --locked --lib\"",
            "echo \"quoted \\\" cargo test --locked --all-targets\"",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line),
                Vec::<String>::new(),
                "{line} is one `echo`; the escaped quote is data, not the end of the span"
            );
        }
        // In `'` there are no escapes, so a backslash is data and the
        // span still ends at the next `'`. Asserted so the conditional
        // is a decision rather than an oversight.
        assert_eq!(
            runs_with_overflow_checks("echo 'a \\' && cargo test --locked --lib").len(),
            1,
            "single quotes have no escapes: the span ends and a real run follows"
        );
    }

    /// AN `&&` LIST ONLY CARRIES ITS FAILURE IF IT ENDS THE LINE.
    ///
    /// Measured, because it is the opposite of what the shape suggests:
    ///
    ///     bash -e -c 'false && echo x'                exit 1
    ///     bash -e -c 'false && echo x; echo REACHED'  exit 0, prints
    ///
    /// `set -e` exempts a command inside an `&&` list, and the list's
    /// own failure does not abort either -- so whatever follows runs
    /// and its status becomes the line's. A gate written that way
    /// cannot fail.
    #[test]
    fn an_and_list_that_does_not_end_the_line_has_its_failure_swallowed() {
        for line in [
            "cargo test --locked --lib && echo ok; echo done",
            "cargo test --locked --all-targets && echo ok; ls",
            // A NEWLINE IS THE SPELLING A WORKFLOW ACTUALLY USES, and
            // the two above cannot reach it: both put the following
            // command on the same line, so "ends the line" was true
            // and the rule said read. Measured:
            // `bash -e -c $'false && echo x\necho R'` exits 0.
            "cargo test --locked --all-targets && echo done\necho \"second line\"\n",
            "cargo test --locked --lib && echo ok\ncargo build --locked\n",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line),
                Vec::<String>::new(),
                "{line}: the && list is followed by another command, so its status is replaced"
            );
        }
    }

    /// THE ACCEPTANCE HALF, and it is the common spelling: an `&&` list
    /// that ends the line does carry the failure.
    #[test]
    fn an_and_list_that_ends_the_line_still_counts() {
        for line in [
            "cargo test --locked --lib && echo ok",
            "cargo test --locked --lib && echo ok && echo done",
            "cd .. && cargo test --locked --lib",
            // THE ACCEPTANCE HALF OF THE MULTI-LINE RULE. When the
            // chain IS the last line, its status is the step's, and a
            // fix that keyed on "is a multi-line block" rather than on
            // "is the last line" would refuse this.
            "set -euo pipefail\ncargo test --locked --all-targets && echo ok\n",
            "cd ..\ncargo test --locked --lib && echo ok\n",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line).len(),
                1,
                "{line}: exit 1 propagates, measured with bash -e"
            );
        }
    }

    /// `set +o errexit` ON THE SAME LINE disqualifies the script, the
    /// long spelling as much as the short one. Pinned because it was
    /// raised as a possible hole and is not one.
    #[test]
    fn a_same_line_set_plus_e_still_disqualifies_the_script() {
        for line in [
            "set +o errexit; cargo test --locked --lib",
            "set +e; cargo test --locked --lib",
            "set +ex; cargo test --locked --lib",
        ] {
            assert_eq!(
                runs_with_overflow_checks(line),
                Vec::<String>::new(),
                "{line}: measured -- bash -e -c 'set +o errexit; false; echo R' exits 0"
            );
        }
    }

    /// AND THE DELIBERATE OVER-STRICTNESS BESIDE IT, stated as a test
    /// rather than as a comment.
    ///
    /// A script that turns `errexit` off and back on again is refused
    /// whole. Restoring it does make the later run gate again, so this
    /// refuses a workflow that would work — the file's declared
    /// direction, because tracking where the setting is live means
    /// tracking control flow. It fails loudly with the script quoted,
    /// which is the outcome this file prefers to a silent pass.
    #[test]
    fn errexit_restored_later_is_still_refused_and_that_is_deliberate() {
        let script = "set +e\nsomething || true\nset -e\ncargo test --locked --lib\n";
        assert_eq!(
            runs_with_overflow_checks(script),
            Vec::<String>::new(),
            "over-strict on purpose: the guard does not track where `set -e` is live"
        );
    }

    /// The near miss that the trailing space protects: `--tests` DOES
    /// build the library unit tests and must still count. Without the
    /// space this is excluded and the guard refuses a correct workflow.
    #[test]
    fn a_tests_flag_run_counts_which_is_what_the_trailing_space_protects() {
        assert_eq!(
            runs_with_overflow_checks("cargo test --locked --tests"),
            vec!["cargo test --locked --tests".to_string()],
        );
        assert_eq!(
            runs_with_overflow_checks("cargo test --locked --all-targets").len(),
            1,
            "`--all-targets` builds the library too"
        );
    }
}

/// The handshake half of the shell scanner.
mod handshake {
    use super::debug_runs_that_prove_the_build_traps;

    #[test]
    fn a_debug_run_carrying_the_handshake_counts() {
        let script = "EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n";
        assert_eq!(
            debug_runs_that_prove_the_build_traps(script),
            vec!["EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib".to_string()],
        );
    }

    /// A debug run that exists and asks the build nothing. Buys a
    /// compile and no information.
    #[test]
    fn a_debug_run_without_the_handshake_does_not_count() {
        let script = "cargo test --locked --lib\n";
        assert_eq!(
            debug_runs_that_prove_the_build_traps(script),
            Vec::<String>::new(),
            "the step is there but nothing checks the build it produced"
        );
    }

    /// A handshake on a release run proves nothing and must not satisfy
    /// this: the checks are off in release on purpose.
    #[test]
    fn the_handshake_on_a_release_run_does_not_count() {
        let script = "EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --release\n";
        assert_eq!(
            debug_runs_that_prove_the_build_traps(script),
            Vec::<String>::new(),
        );
    }

    /// And quoted inside a shell comment, which is where a `run: |`
    /// block would explain it.
    #[test]
    fn the_handshake_quoted_in_a_comment_does_not_count() {
        let script = "#     EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n";
        assert_eq!(
            debug_runs_that_prove_the_build_traps(script),
            Vec::<String>::new(),
        );
    }

    /// A PRINTED MENTION IS NOT AN ASSIGNMENT, and it is not a comment
    /// either — which is why stripping comments did not close this.
    ///
    /// `echo "EXPECT_OVERFLOW_CHECKS=1"` on its own line armed the
    /// whole step, so every `cargo test` in it counted as proving the
    /// build traps, while the process received no variable and the
    /// runtime probe returned without asserting anything.
    /// AN ASSIGNMENT WITH NO COMMAND AFTER IT ARMS NOTHING.
    ///
    /// A prefix assignment is exported to the command it prefixes and
    /// to nothing else, so a standalone `EXPECT_OVERFLOW_CHECKS=1` line
    /// sets a SHELL variable that the next line's `cargo test` never
    /// sees. The guard reported a probe that could not have run.
    #[test]
    fn an_assignment_with_no_command_after_it_arms_nothing() {
        for line in [
            "EXPECT_OVERFLOW_CHECKS=1",
            "  EXPECT_OVERFLOW_CHECKS=1  ",
            "env EXPECT_OVERFLOW_CHECKS=1",
            "EXPECT_OVERFLOW_CHECKS=1 OTHER=2",
            // `export` with nothing to export is the same shape.
            "export",
        ] {
            assert!(
                !super::line_assigns_the_handshake(line),
                "{line}: nothing is exported, so no later cargo test can see the handshake"
            );
        }

        // AND THE WHOLE STEP IS NOT ARMED BY ONE. This is the shape a
        // workflow would actually be written in, and it is what
        // `gating_runs_that_prove_the_build_traps` reads.
        let block = "EXPECT_OVERFLOW_CHECKS=1\ncargo test --locked --lib\n";
        assert_eq!(
            super::debug_runs_that_prove_the_build_traps(block),
            Vec::<String>::new(),
            "the assignment is on its own line, so the cargo test runs without it"
        );
    }

    /// ACCEPTANCE FOR THAT, AND IT IS THE HALF A `require export` FIX
    /// WOULD HAVE BROKEN.
    ///
    /// `EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib` is the
    /// spelling this repository's own workflow uses. Requiring `export`
    /// would refuse it — the guard refusing a correct workflow, which
    /// is the failure this file's history is made of.
    #[test]
    fn the_spellings_that_really_export_it_still_arm_the_step() {
        for line in [
            "EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib",
            "env EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib",
            "EXPECT_OVERFLOW_CHECKS=1 RUST_BACKTRACE=1 cargo test --locked --lib",
            "RUST_BACKTRACE=1 EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib",
            "export EXPECT_OVERFLOW_CHECKS=1",
            "export RUST_BACKTRACE=1 EXPECT_OVERFLOW_CHECKS=1",
        ] {
            assert!(
                super::line_assigns_the_handshake(line),
                "{line} really does put the handshake in an environment a child receives"
            );
        }

        // And the printer is still refused, unchanged by any of this.
        assert!(
            !super::line_assigns_the_handshake("echo EXPECT_OVERFLOW_CHECKS=1"),
            "a printed handshake still arms nothing"
        );
    }

    #[test]
    fn a_printed_handshake_does_not_arm_the_step() {
        for line in [
            "echo \"EXPECT_OVERFLOW_CHECKS=1\"",
            "echo EXPECT_OVERFLOW_CHECKS=1",
            "printf '%s\\n' EXPECT_OVERFLOW_CHECKS=1",
            "echo \"setting EXPECT_OVERFLOW_CHECKS=1 for this step\"",
            "grep -q EXPECT_OVERFLOW_CHECKS=1 ci.yml",
        ] {
            assert!(
                !super::line_assigns_the_handshake(line),
                "{line} mentions the handshake; it does not set it"
            );
        }
    }

    /// THE ACCEPTANCE HALF, and it is the larger risk: refusing a real
    /// assignment makes the guard reject a workflow that is doing
    /// exactly what it asks for.
    #[test]
    fn every_spelling_that_really_assigns_it_still_arms_the_step() {
        for line in [
            "EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib",
            "env EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib",
            "RUST_BACKTRACE=1 EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib",
            "export EXPECT_OVERFLOW_CHECKS=1",
            "EXPECT_OVERFLOW_CHECKS=1 cargo +stable test --locked --lib",
        ] {
            assert!(
                super::line_assigns_the_handshake(line),
                "{line} assigns the handshake and must arm the step"
            );
        }
    }

    /// THE SECOND CALL SITE. `debug_runs_that_prove_the_build_traps`
    /// asked the same question with the same `contains`, and fixing
    /// only `step_declares_the_handshake` left the suite green — no
    /// test drove this path with a printed mention on the same line as
    /// a real run.
    #[test]
    fn a_printed_handshake_beside_a_real_run_does_not_prove_anything() {
        for script in [
            "echo \"EXPECT_OVERFLOW_CHECKS=1\" && cargo test --locked --lib\n",
            "cargo test --locked --lib -- --skip EXPECT_OVERFLOW_CHECKS=1\n",
        ] {
            assert_eq!(
                debug_runs_that_prove_the_build_traps(script),
                Vec::<String>::new(),
                "{script:?}: the handshake is mentioned, not assigned, so the process \
                 gets no variable and the runtime probe asserts nothing"
            );
        }
    }

    /// The acceptance half of that call site: the same line shape with a
    /// real assignment still counts.
    #[test]
    fn a_real_assignment_beside_a_run_on_one_line_still_counts() {
        let script = "export EXPECT_OVERFLOW_CHECKS=1 && cargo test --locked --lib\n";
        assert_eq!(debug_runs_that_prove_the_build_traps(script).len(), 1);
    }

    /// The `env:` mapping is untouched by any of this, and it is the
    /// only spelling that works on a matrix including `windows-latest`.
    #[test]
    fn the_env_mapping_still_arms_the_step() {
        let step = super::Step {
            keys: vec!["run".to_string(), "env".to_string()],
            run: "cargo test --locked --lib\n".to_string(),
            env: vec!["EXPECT_OVERFLOW_CHECKS=1".to_string()],
        };
        assert!(super::step_declares_the_handshake(&step));

        let printed = super::Step {
            keys: vec!["run".to_string()],
            run: "echo \"EXPECT_OVERFLOW_CHECKS=1\"\ncargo test --locked --lib\n".to_string(),
            env: Vec::new(),
        };
        assert!(
            !super::step_declares_the_handshake(&printed),
            "a step whose only mention is printed is not armed"
        );
    }
}

/// The manifest scanner, held to the shapes it has to tell apart. These
/// do not depend on this repository's own `Cargo.toml`, so they keep
/// meaning something after it changes.
mod manifest_parser {
    use super::profiles_disabling_overflow_checks;

    #[test]
    fn the_test_profile_disabling_the_checks_is_caught() {
        let manifest = "\
[profile.release]
lto = true

[profile.test]
overflow-checks = false
";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            vec!["profile.test.overflow-checks".to_string()],
        );
    }

    /// This repository has an explicit `[profile.dev]`, so this is the
    /// likeliest place the setting would actually arrive.
    #[test]
    fn the_dev_profile_disabling_the_checks_is_caught() {
        let manifest = "[profile.dev]\nopt-level = 1\noverflow-checks   =   false\n";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            vec!["profile.dev.overflow-checks".to_string()],
        );
    }

    /// Release is expected to have them off. Flagging it would make the
    /// guard fail on every correct manifest, which is the fastest way
    /// to get a guard deleted.
    #[test]
    fn the_release_profile_disabling_the_checks_is_not_flagged() {
        let manifest = "[profile.release]\noverflow-checks = false\n";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            Vec::<String>::new(),
        );
    }

    /// A commented-out line is not a setting -- the same trap as the
    /// workflow parser's, in the other file this module reads.
    #[test]
    fn a_commented_out_setting_is_not_a_setting() {
        let manifest = "[profile.test]\n# overflow-checks = false\nopt-level = 1\n";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            Vec::<String>::new(),
        );
    }

    /// The comment strip, which nothing else here pins. The realistic
    /// way this setting arrives is with its excuse on the same line,
    /// and it must still be caught: unstripped, the value reads
    /// `false  # speeds the suite up`, which is not `false`, and the
    /// guard waves through the exact edit it exists to catch.
    #[test]
    fn a_disabling_line_with_a_trailing_comment_is_still_caught() {
        let manifest = "[profile.test]\noverflow-checks = false  # speeds the suite up\n";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            vec!["profile.test.overflow-checks".to_string()],
        );
    }

    /// A different setting being `false` is not this setting being
    /// `false`. Without this the scanner could be keying on the value
    /// alone -- flagging any `= false` under those two sections -- and
    /// every other test here would still pass.
    #[test]
    fn another_setting_being_false_is_not_this_one() {
        let manifest = "[profile.test]\ndebug-assertions = false\nopt-level = 1\n";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            Vec::<String>::new(),
        );
    }

    /// THE SPELLINGS THAT DEFEATED THE FIRST VERSION. Each of these was
    /// measured to genuinely switch the checks off, with no warning
    /// from cargo -- see the table on
    /// `profiles_disabling_overflow_checks`. A guard that reads one
    /// spelling of a setting is a guard against typing it one way.
    #[test]
    fn a_double_quoted_key_is_the_same_key() {
        let manifest = "[profile.test]\n\"overflow-checks\" = false\n";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            vec!["profile.test.overflow-checks".to_string()],
        );
    }

    #[test]
    fn a_literal_quoted_key_is_the_same_key() {
        let manifest = "[profile.dev]\n'overflow-checks' = false\n";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            vec!["profile.dev.overflow-checks".to_string()],
        );
    }

    /// The one a section-matching scan cannot see at all: the profile
    /// name is on the key side, so the section is only `profile`.
    #[test]
    fn a_dotted_key_putting_the_profile_on_the_key_side_is_caught() {
        let manifest = "[profile]\ntest.overflow-checks = false\n";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            vec!["profile.test.overflow-checks".to_string()],
        );
    }

    /// And with no section header at all, which is still valid TOML.
    #[test]
    fn a_top_level_dotted_key_is_caught() {
        let manifest = "profile.test.overflow-checks = false\n";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            vec!["profile.test.overflow-checks".to_string()],
        );
    }

    #[test]
    fn a_quoted_section_is_the_same_section() {
        let manifest = "[\"profile\".'test']\noverflow-checks = false\n";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            vec!["profile.test.overflow-checks".to_string()],
        );
    }

    /// Release stays exempt in the dotted spelling too, or normalising
    /// the path would have quietly widened what the guard refuses.
    #[test]
    fn the_release_profile_is_exempt_in_the_dotted_spelling_too() {
        let manifest = "[profile]\nrelease.overflow-checks = false\n";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            Vec::<String>::new(),
        );
    }

    /// `true` is the state we want and must not be reported as the
    /// state we do not. Without this the scanner could be keying on the
    /// word `overflow-checks` alone and nothing here would notice.
    #[test]
    fn enabling_the_checks_explicitly_is_not_flagged() {
        let manifest = "[profile.test]\noverflow-checks = true\n";
        assert_eq!(
            profiles_disabling_overflow_checks(manifest),
            Vec::<String>::new(),
        );
    }
}

/// WHAT ELSE DECIDES WHETHER THE STEP GATES -- one test per item on the
/// enumerated list, because each is a separate way for the gate to go
/// blind with the command still present and still matching.
///
/// The version of this guard these replace matched the `- run:` line in
/// isolation. Measured against this repository's own workflow, `if:
/// false` and `continue-on-error: true` each left all 31 of its tests
/// green while the gate stopped gating.
mod gating {
    use super::gating_runs_that_prove_the_build_traps;

    /// A HANDSHAKE IN A SHELL COMMENT DOES NOT ARM THE STEP.
    ///
    /// `step_declares_the_handshake` read `step.run` verbatim while the
    /// command scan stripped comments, so this workflow satisfied both
    /// assertions and handed the process no `EXPECT_OVERFLOW_CHECKS` at
    /// all -- leaving the runtime probe to return without asserting
    /// anything.
    ///
    /// The existing `handshake::the_handshake_quoted_in_a_comment_does_not_count`
    /// looks like this test and is not: it exercises the script-level
    /// path, which was already comment-stripped, and its fixture has no
    /// real command after the comment, so it could not diverge on the
    /// defect even pointed at the right function.
    #[test]
    fn a_handshake_in_a_shell_comment_does_not_arm_a_step() {
        for block in [
            "      - run: |\n          # EXPECT_OVERFLOW_CHECKS=1 -- see ci_profile.rs\n          cargo test --locked --lib\n",
            "      - run: |\n          cargo test --locked --lib  # EXPECT_OVERFLOW_CHECKS=1 is set in CI\n",
        ] {
            let yaml = GATING.replace(
                "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
                block,
            );
            assert!(
                gating_runs_that_prove_the_build_traps(&yaml).is_empty(),
                "the variable is named in a comment, so the process never receives \
                 it and the runtime probe asserts nothing: {block:?}"
            );
        }
    }

    /// A HANDSHAKE GLUED TO A TERMINATOR DOES NOT ARM THE STEP EITHER.
    ///
    /// `command_lines` cut the comment at `" #"`, which is the rule
    /// "a `#` that begins a word" written for exactly one of the
    /// characters that end a word. The three it missed are the ones
    /// `shell_commands` already splits on, so the two readers of one
    /// text had two grammars again -- the defect this file records
    /// having fixed once already, at a different character.
    ///
    /// `&&#` is not valid bash, and is here anyway: the guard must not
    /// depend on the evasion being a shape bash would accept, and
    /// over-strict is the safe direction everywhere in this file.
    #[test]
    fn a_handshake_glued_to_a_terminator_does_not_arm_a_step() {
        for block in [
            "      - run: |\n          cargo test --locked --lib;# EXPECT_OVERFLOW_CHECKS=1 is set in CI\n",
            "      - run: |\n          (cargo test --locked --lib)# EXPECT_OVERFLOW_CHECKS=1 is set in CI\n",
            "      - run: |\n          cargo test --locked --lib && echo ok&&# EXPECT_OVERFLOW_CHECKS=1 is set in CI\n",
        ] {
            let yaml = GATING.replace(
                "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
                block,
            );
            assert!(
                gating_runs_that_prove_the_build_traps(&yaml).is_empty(),
                "the variable is named in a comment, so the process never receives \
                 it and the runtime probe asserts nothing: {block:?}"
            );
        }
    }

    /// THE ACCEPTANCE HALF: both real spellings still arm the step.
    ///
    /// The `env:` mapping is not a concession -- it is the only
    /// spelling that works on a matrix including `windows-latest`.
    #[test]
    fn both_real_spellings_of_the_handshake_still_arm_a_step() {
        for block in [
            "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
            "      - run: cargo test --locked --lib\n        env:\n          EXPECT_OVERFLOW_CHECKS: \"1\"\n",
            "      - run: |\n          # the guard is armed below, not here\n          EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
        ] {
            let yaml = GATING.replace(
                "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
                block,
            );
            assert_eq!(
                gating_runs_that_prove_the_build_traps(&yaml).len(),
                1,
                "this step really does ask the build to prove it traps: {block:?}"
            );
        }
    }

    /// The shape that does gate, as a control. Every test below is this
    /// with one thing added, so a failure here would mean the fixture
    /// is wrong rather than the property.
    const GATING: &str = "\
on:
  pull_request:
    branches: [main]
jobs:
  test:
    steps:
      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib
";

    #[test]
    fn the_control_shape_gates() {
        assert_eq!(
            gating_runs_that_prove_the_build_traps(GATING).len(),
            1,
            "the control must be counted, or every test below passes for the wrong reason"
        );
    }

    #[test]
    fn a_step_carrying_if_does_not_gate() {
        for condition in [
            "if: false",
            "if: ${{ false }}",
            "if: github.event_name == 'push'",
            "if: ${{ env.SOMETHING == 'yes' }}",
        ] {
            let yaml = GATING.replace(
                "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
                &format!(
                    "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n        {condition}\n"
                ),
            );
            assert!(
                gating_runs_that_prove_the_build_traps(&yaml).is_empty(),
                "a step carrying `{condition}` may or may not run, so it cannot be what \
                 makes the gate able to see an overflow. Rejected on the key's presence \
                 rather than by evaluating it -- the spellings are open-ended."
            );
        }
    }

    #[test]
    fn a_step_carrying_continue_on_error_does_not_gate() {
        let yaml = GATING.replace(
            "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
            "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n        continue-on-error: true\n",
        );
        assert!(
            gating_runs_that_prove_the_build_traps(&yaml).is_empty(),
            "the step runs and its failure is discarded, which is the project's own named \
             defect: a step that runs and whose result nothing reads"
        );
    }

    #[test]
    fn a_job_carrying_if_does_not_gate() {
        let yaml = GATING.replace("  test:\n", "  test:\n    if: false\n");
        assert!(
            gating_runs_that_prove_the_build_traps(&yaml).is_empty(),
            "the same reasoning one level up: a job that may not run cannot gate"
        );
    }

    #[test]
    fn a_job_carrying_continue_on_error_does_not_gate() {
        let yaml = GATING.replace("  test:\n", "  test:\n    continue-on-error: true\n");
        assert!(
            gating_runs_that_prove_the_build_traps(&yaml).is_empty(),
            "a job whose failure is discarded cannot gate, however sound its steps"
        );
    }

    /// The assumption the `ci.yml`-only scan rests on, which is a fact
    /// about the file rather than a given.
    #[test]
    fn a_workflow_that_no_longer_runs_on_pull_request_does_not_gate() {
        let yaml = GATING.replace(
            "  pull_request:\n    branches: [main]\n",
            "  push:\n    branches: [main]\n",
        );
        assert!(
            gating_runs_that_prove_the_build_traps(&yaml).is_empty(),
            "scoping the scan to ci.yml assumes ci.yml is what runs on a pull request; if its \
             triggers stop including pull_request, the step gates nothing no matter how it looks"
        );
    }

    /// A `run: |` block is read whole, so a command inside a loop is
    /// visible. This repository has TWO such loops in kernel-gate, and
    /// a line-range extraction drops the second.
    #[test]
    fn a_run_block_is_read_whole() {
        let yaml = "\
on:
  pull_request:
    branches: [main]
jobs:
  test:
    steps:
      - name: a block
        run: |
          set -euo pipefail
          EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib
";
        assert_eq!(
            gating_runs_that_prove_the_build_traps(yaml).len(),
            1,
            "a command inside a `run: |` block must be seen; the kernel-gate loops live in \
             blocks like this one"
        );
    }

    /// THE QUOTED SPELLINGS, WHICH WERE SILENT DEFEATS. Measured on
    /// `main` at `57cf1b6`: `if: false` correctly turned the suite red,
    /// and `"if": false` -- the same key, quoted -- left all 34 tests
    /// green while Actions skipped the step. The old parser took its
    /// key as `cur.split(':').next()` with no un-quoting, so the key
    /// read `"if"` and matched no entry in `NON_GATING_KEYS`.
    ///
    /// Nothing un-quotes anything now: the key arrives from the parser
    /// already resolved, so every spelling of it is the same key by
    /// construction.
    #[test]
    fn a_quoted_key_is_the_same_key() {
        for spelling in [
            "\"if\": false",
            "'if': false",
            "\"continue-on-error\": true",
            "'continue-on-error': true",
        ] {
            let yaml = GATING.replace(
                "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
                &format!(
                    "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n        {spelling}\n"
                ),
            );
            assert!(
                gating_runs_that_prove_the_build_traps(&yaml).is_empty(),
                "`{spelling}` is the same key as its bare spelling; quoting it must not \
                 make a skipped step count as the thing gating the merge"
            );
        }
    }

    /// And one level up, on the job.
    #[test]
    fn a_quoted_key_on_the_job_is_the_same_key() {
        for spelling in ["\"if\": false", "\"continue-on-error\": true"] {
            let yaml = GATING.replace("  test:\n", &format!("  test:\n    {spelling}\n"));
            assert!(
                gating_runs_that_prove_the_build_traps(&yaml).is_empty(),
                "`{spelling}` on the job is the same key as its bare spelling"
            );
        }
    }

    /// THE COMMENTED-OUT TRIGGER, ALSO A SILENT DEFEAT. The old check
    /// asked whether the `on:` block's raw text -- comments included --
    /// contained the characters `pull_request`, so commenting the
    /// trigger out left the guard green on a workflow that no longer
    /// ran on pull requests at all. Measured on `main`: 34 passed,
    /// both arms.
    #[test]
    fn a_commented_out_pull_request_trigger_does_not_gate() {
        let commented_with_another_trigger_left = GATING.replace(
            "  pull_request:\n    branches: [main]\n",
            "  # pull_request:\n  #   branches: [main]\n  push:\n    branches: [main]\n",
        );
        let only_a_comment_naming_it = GATING.replace(
            "  pull_request:\n    branches: [main]\n",
            "  # pull_request disabled while we investigate flaky runners\n  push:\n    branches: [main]\n",
        );
        for yaml in [
            &commented_with_another_trigger_left,
            &only_a_comment_naming_it,
        ] {
            assert!(
                gating_runs_that_prove_the_build_traps(yaml).is_empty(),
                "a trigger named only in a comment is not a trigger; the parser drops \
                 comments before anything compares a name, so there is no `#` to strip \
                 and none to forget:\n{yaml}"
            );
        }
    }

    /// A whole-name comparison, so a trigger that merely begins with
    /// those characters is a different trigger. `pull_request_review`
    /// fires on a review, not on the pull request, and cannot be what
    /// gates the merge.
    #[test]
    fn a_trigger_that_merely_begins_with_pull_request_does_not_gate() {
        let yaml = GATING.replace(
            "  pull_request:\n    branches: [main]\n",
            "  pull_request_review:\n    types: [submitted]\n",
        );
        assert!(
            gating_runs_that_prove_the_build_traps(&yaml).is_empty(),
            "pull_request_review is not pull_request; a substring match cannot tell \
             them apart and this comparison must"
        );
    }

    /// `pull_request_target` is not `pull_request`, and is refused on
    /// purpose. It runs against the base repository with a write token
    /// and the repository's secrets, and checks out the base ref by
    /// default, so a workflow triggered only that way may never build
    /// the contributor's code. `rust-fs-xfs#146` and
    /// `rust-fs-ext4#149` record it as a live gap in the hand-rolled
    /// guard this file replaces.
    ///
    /// Pinned as a test rather than left to the comparison, because the
    /// clause is one line and was previously written by hand and copied
    /// between repositories. This is what stops it coming back.
    #[test]
    fn pull_request_target_does_not_gate() {
        let yaml = GATING.replace(
            "  pull_request:\n    branches: [main]\n",
            "  pull_request_target:\n    branches: [main]\n",
        );
        assert!(
            gating_runs_that_prove_the_build_traps(&yaml).is_empty(),
            "pull_request_target runs with the base repository's token and secrets \
             and checks out the base ref; it is not proof that the merge is gated"
        );
    }

    /// `on:` may be a sequence of names rather than a mapping, in
    /// either the flow or the block spelling, and all three are
    /// ordinary workflows.
    #[test]
    fn a_sequence_of_triggers_is_read() {
        for spelling in [
            "on: [push, pull_request]\n",
            "on:\n  - push\n  - pull_request\n",
        ] {
            let yaml = GATING.replace("on:\n  pull_request:\n    branches: [main]\n", spelling);
            assert_eq!(
                gating_runs_that_prove_the_build_traps(&yaml).len(),
                1,
                "this workflow triggers on a pull request as surely as the mapping \
                 spelling does:\n{yaml}"
            );
        }
    }

    /// THE ARM THAT WAS A FALSE ALARM RATHER THAN A DEFEAT, AND SO
    /// CANNOT BE WITNESSED BY THE SUITE GOING RED -- it already did.
    /// The witness is that legal YAML now passes.
    ///
    /// The old parser treated only a bare `|` as a block opener
    /// (`after != "|"`), so `|-`, `|+`, `>`, `>-` and `|2` were read as
    /// the command itself and the block's contents never parsed at all.
    /// Measured on `main`: `run: |` 34 passed, `run: |-` and `run: >`
    /// each EXIT=101 with 2 failed -- the guard refusing a completely
    /// correct workflow, which is the fastest way to get a guard
    /// deleted.
    ///
    /// A parser knows all five styles because they are the grammar.
    #[test]
    fn every_block_scalar_style_is_read_whole() {
        for style in ["|", "|-", "|+", ">", ">-", "|2"] {
            let yaml = GATING.replace(
                "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
                &format!(
                    "      - name: a block\n        run: {style}\n          EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n"
                ),
            );
            assert_eq!(
                gating_runs_that_prove_the_build_traps(&yaml).len(),
                1,
                "`run: {style}` is a legal block scalar carrying the gating command; \
                 failing here is the guard refusing a correct workflow:\n{yaml}"
            );
        }
    }

    /// A command quoted in a YAML comment is not a run. This used to be
    /// the shell scanner's job and is the parser's now: comments do not
    /// survive parsing, so there is no `#` handling here to get wrong.
    /// It is asserted at this level because that is where the property
    /// now lives -- `ci.yml` really does quote the gating command
    /// verbatim in the comment block above it, so a scan that missed
    /// this would stay green after the step itself was deleted.
    #[test]
    fn a_debug_run_quoted_in_a_yaml_comment_does_not_gate() {
        let yaml = "\
on:
  pull_request:
    branches: [main]
jobs:
  test:
    steps:
      # Do not remove this as a duplicate of the runs above it:
      #     - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib
      - run: cargo test --locked --release
";
        assert!(
            gating_runs_that_prove_the_build_traps(yaml).is_empty(),
            "the gating command appears only inside a comment, and the step that \
             remains is a release run"
        );
    }

    /// A workflow the parser cannot read is a failure, never a pass.
    /// The direction matters: a guard that swallowed the error and
    /// returned an empty structure would report "no debug run gates
    /// this", which is also a failure and therefore safe -- but one
    /// that returned early with a pass would be the blindness this
    /// whole module exists to refuse.
    #[test]
    #[should_panic(expected = "not valid YAML")]
    fn a_workflow_that_does_not_parse_is_a_failure() {
        super::parse_workflow("jobs:\n  test:\n   - broken: [unclosed\n");
    }

    /// THE CONTROL THAT STOPS THE REFUSAL OVER-CORRECTING.
    ///
    /// `pull_request_target` is refused as insufficient ON ITS OWN.
    /// That is not the same as refusing any workflow that mentions it,
    /// and until this test existed nothing in the file could tell the
    /// two apart: every fixture carried at most one trigger, so this
    /// mutation survived the whole suite --
    ///
    /// ```text
    ///   any(t == "pull_request")
    ///       && !any(t == "pull_request_target")
    /// ```
    ///
    /// -- while refusing a perfectly gated workflow. Carrying both
    /// triggers is the ordinary way to reach repository secrets from a
    /// job without giving up the pull-request gate, and such a workflow
    /// IS gated, by its `pull_request:` key.
    ///
    /// An assertion whose result does not depend on the thing it claims
    /// to check is this project's own recurring defect; this one was in
    /// the test pinning the refusal rather than in the refusal itself.
    #[test]
    fn a_workflow_carrying_both_triggers_still_gates() {
        let yaml = GATING.replace(
            "  pull_request:\n    branches: [main]\n",
            "  pull_request:\n    branches: [main]\n  pull_request_target:\n    branches: [main]\n",
        );
        assert_ne!(yaml, GATING, "the mutation must actually apply");
        assert_eq!(
            gating_runs_that_prove_the_build_traps(&yaml).len(),
            1,
            "the workflow still triggers on pull_request, so it still gates; refusing it \
             because pull_request_target is also present would be the over-correction"
        );
    }

    /// THE HANDSHAKE MAY BE DECLARED IN THE STEP'S `env:` MAPPING.
    ///
    /// Not a concession: on a matrix including `windows-latest` it is
    /// the only spelling that works, because an inline
    /// `VAR=1 cargo test` prefix is bash syntax and a PowerShell syntax
    /// error. A guard that read only the command would refuse the
    /// correct workflow on every cross-platform crate here -- the loud
    /// direction, but wrong, and the fastest way to get a guard
    /// deleted.
    #[test]
    fn the_handshake_declared_in_an_env_mapping_counts() {
        for value in ["\"1\"", "1", "'1'"] {
            let yaml = GATING.replace(
                "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
                &format!(
                    "      - run: cargo test --locked --lib\n        env:\n          EXPECT_OVERFLOW_CHECKS: {value}\n"
                ),
            );
            assert_eq!(
                gating_runs_that_prove_the_build_traps(&yaml).len(),
                1,
                "`EXPECT_OVERFLOW_CHECKS: {value}` in an env mapping is the same \
                 instruction to Actions as the inline prefix, and on a Windows \
                 matrix it is the only one that works:\n{yaml}"
            );
        }
    }

    /// And it must not rescue a `--release` run. The checks are off in
    /// release deliberately, so a handshake there arms an assertion
    /// that would fire on every green run.
    #[test]
    fn the_handshake_in_an_env_mapping_does_not_count_on_a_release_run() {
        let yaml = GATING.replace(
            "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
            "      - run: cargo test --locked --release --lib\n        env:\n          EXPECT_OVERFLOW_CHECKS: \"1\"\n",
        );
        assert!(
            gating_runs_that_prove_the_build_traps(&yaml).is_empty(),
            "a release run cannot prove the build traps, however it is labelled"
        );
    }

    /// A step carrying an `env:` mapping is still a gating step. Over-
    /// strictness here would cost something real: the handshake itself
    /// lives in such a mapping.
    #[test]
    fn a_step_carrying_an_env_mapping_still_gates() {
        let yaml = GATING.replace(
            "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
            "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n        env:\n          SOMETHING_ELSE: \"1\"\n",
        );
        assert_eq!(
            gating_runs_that_prove_the_build_traps(&yaml).len(),
            1,
            "`env:` says nothing about whether the step's result is read"
        );
    }
}
