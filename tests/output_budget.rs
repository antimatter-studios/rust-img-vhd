//! `scripts/output-budget.sh` keeps its side of the bargain.
//!
//! Every test run in this repository goes through that script, and the whole
//! quiet-by-default arrangement rests on five promises it makes:
//!
//! 1. a run that PASSES prints one verdict line and no output;
//! 2. a run that FAILS prints the tail of its log and exits with the
//!    COMMAND's status, not the wrapper's;
//! 3. a run that passed while printing more than its budget FAILS, with a
//!    status of its own (65) so it is not mistaken for a red suite;
//! 4. nothing is lost: the whole run is in the log either way;
//! 5. `--verbose` streams the run and does NOT lift the budget.
//!
//! # WHY THIS FILE EXISTS AT ALL
//!
//! The script is a VENDORED COPY of fs-linux-test-harness's, at v0.1.0 --
//! the header of `scripts/output-budget.sh` says why a copy rather than the
//! sibling the filesystem crates read it from. A copy can drift, and the
//! honest answer to that is to guard the BEHAVIOUR rather than the bytes: a
//! byte comparison would need the sibling on disk, which would mean either a
//! prerequisite this crate's contract does not have or a test that quietly
//! passes when the sibling is absent -- and a check that skips is a check
//! that reports protection it is not providing.
//!
//! A rewrite of the script that still satisfies the five promises is a fine
//! copy. One that does not fails here, rather than in whatever reads the log
//! next.
//!
//! The other half of the arrangement -- that every tier in `ci.yml` and
//! `chores.yml` actually GOES through the script, under a non-zero budget,
//! with the two files agreeing on the numbers -- is in `tests/ci_profile.rs`,
//! which is the file that already parses both.

use std::path::PathBuf;
use std::process::{Command, Output};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The `bash` that can actually run a shell script.
///
/// # `bash` ON PATH IS NOT BASH ON A WINDOWS RUNNER
///
/// `C:\Windows\System32\bash.exe` is the WSL launcher, it ships with the
/// operating system, and `System32` comes early in `PATH` -- so
/// `Command::new("bash")` finds it before Git Bash. With no WSL
/// distribution installed it prints nothing useful and exits 1, which is
/// how every test in this file failed on `windows-latest` while the same
/// scripts ran perfectly in the workflow: an Actions step that says
/// `shell: bash` is handed Git Bash by name and never consults `PATH`.
///
/// So this asks for Git Bash by name on Windows and falls back to `PATH`
/// elsewhere -- and, if that file is not there, still falls back to `PATH`
/// rather than deciding the host cannot run the suite.
fn bash() -> PathBuf {
    if cfg!(windows) {
        let git_bash = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
        if git_bash.is_file() {
            return git_bash;
        }
    }
    PathBuf::from("bash")
}

/// Run `scripts/output-budget.sh` from the repository root.
///
/// FROM THE ROOT, WITH RELATIVE PATHS, because this suite runs on
/// windows-latest, where `bash` is Git Bash and an absolute Windows path
/// handed to it as an argument is a path with backslashes in it. A relative
/// path plus a working directory is the one spelling that means the same
/// thing on all three runners.
fn run_budget(arguments: &[&str], verbose: bool) -> Output {
    let mut command = Command::new(bash());
    command
        .current_dir(repo())
        .arg("scripts/output-budget.sh")
        .args(arguments);
    if verbose {
        command.env("FLTH_VERBOSE", "1");
    }
    command.output().unwrap_or_else(|e| {
        panic!(
            "could not run `{} scripts/output-budget.sh`: {e}. Every test \
             tier in this repository runs through that script, so a host \
             without `bash` cannot run this suite -- which is why this is a \
             failure and not a skip. On Windows, Git Bash provides it.",
            bash().display()
        )
    })
}

/// A scratch log path under `tmp/`, which is gitignored and is where the
/// tier logs live.
fn log_path(name: &str) -> String {
    let directory = repo().join("tmp").join("output-budget-test");
    std::fs::create_dir_all(&directory).expect("could not create tmp/output-budget-test");
    let _ = std::fs::remove_file(directory.join(name));
    format!("tmp/output-budget-test/{name}")
}

fn contents(relative: &str) -> String {
    std::fs::read_to_string(repo().join(relative))
        .unwrap_or_else(|e| panic!("the run left no log at {relative}: {e}"))
}

/// A command that prints `count` lines and succeeds.
const LOUD: &str = "i=0; while [ $i -lt 40 ]; do echo line $i; i=$((i+1)); done";

#[test]
fn a_passing_run_inside_its_budget_prints_a_verdict_and_nothing_else() {
    let log = log_path("quiet.log");
    let output = run_budget(
        &[
            "--log",
            &log,
            "--max-lines",
            "5",
            "--label",
            "quiet",
            "--",
            "echo",
            "hello",
        ],
        false,
    );

    assert!(
        output.status.success(),
        "a run inside its budget failed: {:?}",
        output.status
    );
    let printed = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(
        !printed.contains("hello"),
        "a passing run put the command's output on the terminal, which is the \
         whole thing this script exists to stop:\n{printed}"
    );
    assert!(
        printed.contains("quiet: ok"),
        "a passing run printed no verdict line, so the reader is told nothing \
         at all -- quiet is not the same as silent:\n{printed}"
    );
    assert!(
        printed.contains(&log),
        "the verdict line does not name the log, so the output that was \
         withheld cannot be found:\n{printed}"
    );
    assert_eq!(
        contents(&log).trim(),
        "hello",
        "the log did not keep the command's output"
    );
}

#[test]
fn a_run_that_passed_but_printed_too_much_fails_with_its_own_status() {
    let log = log_path("loud.log");
    let output = run_budget(
        &[
            "--log",
            &log,
            "--max-lines",
            "5",
            "--label",
            "loud",
            "--",
            "sh",
            "-c",
            LOUD,
        ],
        false,
    );

    assert_eq!(
        output.status.code(),
        Some(65),
        "a run over its line budget must exit 65 -- a status of its own, so a \
         suite that printed too much is not mistaken for a suite that failed. \
         It exited {:?}.",
        output.status.code()
    );
    let printed = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        printed.contains("printed 40 lines (budget 5)"),
        "the breach did not say what was printed or what was allowed, so \
         nobody can tell whether to quiet the run or raise the \
         budget:\n{printed}"
    );
    assert_eq!(
        contents(&log).lines().count(),
        40,
        "a breached budget lost the output. The budget is not a gag: \
         everything goes to the log whatever happens to the terminal."
    );
}

#[test]
fn a_breached_byte_budget_fails_the_run_too() {
    let log = log_path("fat.log");
    let output = run_budget(
        &[
            "--log",
            &log,
            "--max-bytes",
            "10",
            "--label",
            "fat",
            "--",
            "echo",
            "rather more than ten bytes",
        ],
        false,
    );

    assert_eq!(
        output.status.code(),
        Some(65),
        "a run over its BYTE budget passed. Lines and bytes are two budgets \
         because one long line is not one line's worth of reading."
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("bytes (budget 10)"),
        "the byte breach did not name the counts"
    );
}

#[test]
fn a_failing_run_prints_the_tail_and_hands_on_the_commands_own_status() {
    let log = log_path("bad.log");
    let output = run_budget(
        &[
            "--log",
            &log,
            "--max-lines",
            "5",
            "--tail",
            "3",
            "--label",
            "bad",
            "--",
            "sh",
            "-c",
            "echo the reason; exit 7",
        ],
        false,
    );

    assert_eq!(
        output.status.code(),
        Some(7),
        "a failing run must exit with the COMMAND's status, not the \
         wrapper's. This is the `cargo test | tee log` defect that \
         tests/ci_profile.rs refuses in the workflow -- a red suite reading \
         green because something else in the pipeline succeeded."
    );
    let printed = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        printed.contains("the reason"),
        "a failure printed no excerpt. A quiet suite is only affordable if a \
         failure still shows its reason without anyone fetching a \
         file:\n{printed}"
    );
}

#[test]
fn a_failing_run_over_its_budget_still_reports_the_failure() {
    // Both things are wrong at once, and the suite's own status is the one
    // that matters: 65 means "passed, but too loud", and this did not pass.
    let log = log_path("bad-and-loud.log");
    let output = run_budget(
        &[
            "--log",
            &log,
            "--max-lines",
            "5",
            "--label",
            "both",
            "--",
            "sh",
            "-c",
            &format!("{LOUD}; exit 3"),
        ],
        false,
    );

    assert_eq!(
        output.status.code(),
        Some(3),
        "a run that failed AND printed too much reported the budget rather \
         than the failure. 65 is reserved for a run that PASSED; a red suite \
         must keep its own status."
    );
}

#[test]
fn verbose_streams_the_run_and_does_not_lift_the_budget() {
    let log = log_path("verbose.log");
    let output = run_budget(
        &[
            "--log",
            &log,
            "--max-lines",
            "5",
            "--label",
            "v",
            "--",
            "echo",
            "hello",
        ],
        true,
    );
    let printed = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        printed.contains("hello"),
        "FLTH_VERBOSE=1 did not stream the run. The quiet default is only \
         acceptable because asking for everything is one variable \
         away:\n{printed}"
    );

    let log = log_path("verbose-loud.log");
    let output = run_budget(
        &[
            "--log",
            &log,
            "--max-lines",
            "5",
            "--label",
            "v",
            "--",
            "sh",
            "-c",
            LOUD,
        ],
        true,
    );
    assert_eq!(
        output.status.code(),
        Some(65),
        "--verbose lifted the budget. It must not: the log is the same size \
         whether or not anybody was watching, and a tier that has outgrown \
         its budget should say so either way."
    );
}

#[test]
fn a_verbose_run_takes_the_commands_status_and_not_the_pipelines() {
    // The verbose path is a PIPELINE -- `"$@" 2>&1 | tee "$LOG"` -- and
    // `tee` succeeds while the command fails. Reading the pipeline's status
    // instead of PIPESTATUS[0] is precisely the `cmd | tee log` defect, and
    // it would be invisible: verbose runs are the ones a person is watching,
    // so the failure is on screen while the status says green.
    let log = log_path("verbose-fail.log");
    let output = run_budget(
        &[
            "--log",
            &log,
            "--max-lines",
            "5",
            "--label",
            "v",
            "--",
            "sh",
            "-c",
            "exit 9",
        ],
        true,
    );
    assert_eq!(
        output.status.code(),
        Some(9),
        "a verbose run that failed reported `tee`'s status instead of the \
         command's."
    );
}

#[test]
fn a_budget_of_zero_is_no_budget_which_is_why_the_workflow_may_not_use_one() {
    // Documented here because it is the reason
    // `every_output_budget_is_a_number_that_can_fail_a_run` in
    // tests/ci_profile.rs exists: a tier given `0` looks wrapped, logged and
    // compliant, and is unbounded.
    let log = log_path("zero.log");
    let output = run_budget(
        &[
            "--log",
            &log,
            "--max-lines",
            "0",
            "--label",
            "zero",
            "--",
            "sh",
            "-c",
            LOUD,
        ],
        false,
    );
    assert!(
        output.status.success(),
        "0 is meant to mean \"no budget\"; if that ever changes, the guard in \
         tests/ci_profile.rs that refuses a zero budget is measuring the \
         wrong thing and should change with it."
    );
}
