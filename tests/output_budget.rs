//! `scripts/tier.sh` resolves rust-fs-core's output-budget wrapper, and
//! refuses anything else.
//!
//! Every test run in this repository goes through `scripts/tier.sh`, and the
//! whole quiet-by-default arrangement rests on the wrapper it runs them
//! under: a passing run prints one verdict line and keeps the transcript, a
//! failing run hands on the command's own status, and a run that passed
//! while printing more than its budget fails with a status of its own (65)
//! so it is not mistaken for a red suite.
//!
//! # WHAT CHANGED, AND WHAT THIS FILE NOW GUARDS
//!
//! The wrapper used to be `scripts/output-budget.sh`, a VENDORED COPY of
//! fs-linux-test-harness's, and this file pinned that copy's behaviour. The
//! copy is gone: the canonical script lives in rust-fs-core and `tier.sh`
//! resolves it at run time (antimatter-studios/rust-fs-core#153). Keeping
//! the old tests would have meant asserting core's behaviour from the wrong
//! repository -- core tests its own script, and a second suite doing it here
//! only decides which repository a change has to be made in twice.
//!
//! What is OURS is the RESOLVER, so that is what these tests cover:
//!
//! 1. a tier runs its command through a wrapper `tier.sh` resolved, a
//!    passing tier is quiet and names its log, and nothing is lost;
//! 2. a failing tier hands on the COMMAND's status, not the wrapper's;
//! 3. a breached budget is its own failure, 65;
//! 4. `--verbose` reaches the canonical variable and does not lift the
//!    budget -- the rename from `FLTH_VERBOSE` to `OUTPUT_BUDGET_VERBOSE`
//!    fails SILENTLY, so it needs a test rather than a reading;
//! 5. an absent core is REFUSED, loudly, naming what would provide it;
//! 6. a core whose `--version` disagrees is REFUSED and nothing else is
//!    tried, because "some other output-budget.sh was found" is how this
//!    family ended up with several;
//! 7. the override wins over the sibling, or it is not an override;
//! 8. the copy the run takes is removed when the run ends;
//! 9. no copy of the wrapper is committed here again.
//!
//! # WHY THE CONTRACT IS `--version` AND NOT A DIGEST
//!
//! rust-fs-ntfs pins a SHA-256 of the file. That was rejected here: a digest
//! pinned in every consumer recreates exactly the lockstep this migration
//! removes -- a one-line fix in core could not be taken by anybody until a
//! commit landed in all of them, and the pressure that produces is to stop
//! fixing the script. `rust-fs-core-output-budget 1` is an API version, it
//! changes when the contract does, and a copy answering anything else is
//! refused outright.
//!
//! # NOTHING HERE SKIPS
//!
//! The tests that need a real core do not check whether one is present and
//! return early: they FAIL, naming `FS_CORE_ROOT` and the `../rust-fs-core`
//! checkout that would provide it. A skipped test reads exactly like a
//! passing one.
//!
//! The other half of the arrangement -- that every tier in `ci.yml` and
//! `chores.yml` actually GOES through `tier.sh`, under a non-zero budget,
//! with the two files agreeing on the numbers -- is in `tests/ci_profile.rs`,
//! which is the file that already parses both.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, OnceLock};

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

/// One tier at a time, so the leftover check below is about THIS run.
///
/// `tier.sh` copies the wrapper to `tmp/output-budget.$$.sh` for the length
/// of the run, and every test here asserts that the copy is gone afterwards.
/// Two tiers running at once would each see the other's copy, and the
/// assertion would be about scheduling rather than about the trap.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = LOCK.get_or_init(|| Mutex::new(()));
    // A panicking test poisons the mutex; the tests after it would then fail
    // for a reason that is not theirs.
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run `scripts/tier.sh` from the repository root, and check it left no copy
/// of the wrapper behind.
///
/// FROM THE ROOT, WITH RELATIVE PATHS, because this suite runs on
/// windows-latest, where `bash` is Git Bash and an absolute Windows path
/// handed to it as an argument is a path with backslashes in it. A relative
/// path plus a working directory is the one spelling that means the same
/// thing on all three runners -- and it is why `tier.sh` resolves a relative
/// `FS_CORE_ROOT` against the repository.
fn tier(arguments: &[&str], environment: &[(&str, &str)]) -> Output {
    let _guard = one_at_a_time();
    let before = wrapper_copies();
    let mut command = Command::new(bash());
    command
        .current_dir(repo())
        .arg("scripts/tier.sh")
        .args(arguments);
    for (name, value) in environment {
        command.env(name, value);
    }
    let output = command.output().unwrap_or_else(|e| {
        panic!(
            "could not run `{} scripts/tier.sh`: {e}. Every test tier in this \
             repository runs through that script, so a host without `bash` \
             cannot run this suite -- which is why this is a failure and not \
             a skip. On Windows, Git Bash provides it.",
            bash().display()
        )
    });
    let left_behind = copies_added_since(&before);
    assert!(
        left_behind.is_empty(),
        "the run left {left_behind:?} in tmp/. tier.sh copies the wrapper to \
         tmp/output-budget.$$.sh for the length of the run and traps EXIT to \
         remove it; a copy that survives is a committed copy in waiting."
    );
    output
}

/// The wrapper copies that appeared while a tier ran.
///
/// A DIFFERENCE AND NOT A COUNT, because this suite runs INSIDE a tier: the
/// outer `tier.sh` that `chore test` and `ci.yml` invoke holds its own
/// `tmp/output-budget.<pid>.sh` open for the whole run, and it is not this
/// run's to sweep up. The mutex above is what makes the difference mean
/// something: no other tier here may start between the two readings.
fn copies_added_since(before: &[String]) -> Vec<String> {
    wrapper_copies()
        .into_iter()
        .filter(|name| !before.contains(name))
        .collect()
}

/// Every `tmp/output-budget.*.sh` presently on disk.
fn wrapper_copies() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(repo().join("tmp")) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
        .filter(|name| name.starts_with("output-budget.") && name.ends_with(".sh"))
        .collect()
}

/// stdout, stderr and the status in one string, for an assertion message.
fn described(output: &Output) -> String {
    format!(
        "status {:?}\n--- stdout\n{}\n--- stderr\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A tier that was expected to RUN, rather than to refuse.
///
/// This is the "nothing skips" clause in one function: a host with no
/// rust-fs-core to resolve does not quietly pass these tests, it fails them
/// with the two things that would fix it.
fn assert_the_wrapper_resolved(output: &Output) {
    let complaint = stderr(output);
    assert!(
        !complaint.contains("tier.sh: no rust-fs-core output-budget wrapper")
            && !complaint.contains("tier.sh: FS_CORE_ROOT is set")
            && !complaint.contains("to --version, not"),
        "tier.sh could not resolve rust-fs-core's output-budget wrapper, so \
         this test could not run the thing it is about. That is a FAILURE and \
         not a skip: provide it by checking rust-fs-core out beside this \
         repository at v0.2.13 or later, or by setting FS_CORE_ROOT to a \
         checkout that has scripts/output-budget.sh.\n{}",
        described(output)
    );
}

/// A scratch directory under `tmp/`, which is gitignored, emptied first so a
/// previous run's marker cannot be mistaken for this one's.
fn scratch(name: &str) -> PathBuf {
    let directory = repo().join("tmp").join("output-budget-test").join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory)
        .unwrap_or_else(|e| panic!("could not create tmp/output-budget-test/{name}: {e}"));
    directory
}

/// A stand-in rust-fs-core whose wrapper answers `version` to `--version`,
/// and which records having been RUN as a wrapper.
///
/// The marker is what tells a refusal apart from a silent acceptance: a
/// resolver that runs it anyway leaves the file, and the test that expects a
/// refusal says so.
fn fake_core(name: &str, version: &str) -> (PathBuf, PathBuf) {
    let root = scratch(name);
    let marker = root.join("ran-as-the-wrapper");
    std::fs::create_dir_all(root.join("scripts"))
        .expect("could not create the fake core's scripts");
    // THE MARKER PATH IS RELATIVE, and it has to be: tier.sh COPIES the
    // wrapper into tmp/ before running it, so anything resolved from
    // `$BASH_SOURCE` names the copy's directory rather than this one -- and
    // an absolute path baked in here would be a backslash path on the
    // Windows leg, which Git Bash cannot open. tier.sh never changes
    // directory, so the repository root is the working directory either way.
    let relative = format!("{}/ran-as-the-wrapper", relative_root(name));
    let script = format!(
        "#!/usr/bin/env bash\n\
         if [ \"${{1:-}}\" = --version ]; then\n\
         \x20   printf '{version}\\n'\n\
         \x20   exit 0\n\
         fi\n\
         : > '{relative}'\n\
         exit 0\n"
    );
    std::fs::write(root.join("scripts").join("output-budget.sh"), script)
        .expect("could not write the fake core's wrapper");
    (root, marker)
}

/// Where a fake core is, spelled the way `tier.sh` wants it: relative to the
/// repository, so Git Bash never sees a `C:\` path.
fn relative_root(name: &str) -> String {
    format!("tmp/output-budget-test/{name}")
}

fn log_of(name: &str) -> PathBuf {
    repo().join("tmp").join("logs").join(format!("{name}.log"))
}

/// 40 lines and a zero status.
const LOUD: &str = "i=0; while [ $i -lt 40 ]; do echo line $i; i=$((i+1)); done";

#[test]
fn a_passing_tier_is_quiet_and_names_its_log() {
    let name = "output-budget-test/quiet";
    let _ = std::fs::remove_file(log_of(name));
    let output = tier(&["quiet", name, "5", "0", "--", "echo", "hush"], &[]);
    assert_the_wrapper_resolved(&output);

    assert!(output.status.success(), "{}", described(&output));
    let verdict = stdout(&output);
    assert!(
        verdict.contains("quiet: ok (") && verdict.contains("quiet.log"),
        "a passing tier should print one verdict line naming its log.\n{}",
        described(&output)
    );
    assert!(
        !verdict.contains("hush"),
        "the tier's own output belongs in the log, not on the terminal.\n{}",
        described(&output)
    );
    let kept = std::fs::read_to_string(log_of(name))
        .expect("a passing tier still keeps the whole run in its log");
    assert!(
        kept.contains("hush"),
        "the log did not keep the run: {kept}"
    );
}

#[test]
fn a_failing_tier_hands_on_the_commands_own_status() {
    let name = "output-budget-test/boom";
    let _ = std::fs::remove_file(log_of(name));
    let output = tier(
        &[
            "boom",
            name,
            "400",
            "0",
            "--",
            "bash",
            "-c",
            "echo the reason; exit 3",
        ],
        &[],
    );
    assert_the_wrapper_resolved(&output);

    assert_eq!(
        output.status.code(),
        Some(3),
        "a failing tier reports the COMMAND's status, not the wrapper's -- a \
         suite that exits 3 must not arrive as a 1.\n{}",
        described(&output)
    );
    let complaint = stderr(&output);
    assert!(
        complaint.contains("FAILED (exit 3)") && complaint.contains("boom.log"),
        "a failure names its status and the log holding the run.\n{}",
        described(&output)
    );
    // Core's wrapper defaults --tail to 0: one line naming the log, not forty
    // lines of it. The vendored copy this replaced printed the tail always,
    // so this assertion is what would notice a resolver that found an old
    // copy somewhere and used it.
    assert!(
        !complaint.contains("the reason"),
        "a failing tier prints a line naming the log, not the log.\n{}",
        described(&output)
    );
    let kept = std::fs::read_to_string(log_of(name)).expect("a failing tier keeps its log too");
    assert!(
        kept.contains("the reason"),
        "nothing is lost on a failure: {kept}"
    );
}

#[test]
fn a_tier_that_breaches_its_budget_exits_65() {
    let output = tier(
        &[
            "loud",
            "output-budget-test/loud",
            "5",
            "0",
            "--",
            "bash",
            "-c",
            LOUD,
        ],
        &[],
    );
    assert_the_wrapper_resolved(&output);

    assert_eq!(
        output.status.code(),
        Some(65),
        "a run that passed while printing more than its budget is a failure \
         of its own, told apart from a red suite by its status.\n{}",
        described(&output)
    );
    assert!(
        stderr(&output).contains("40 lines (budget 5)"),
        "the breach says what it measured and what it was allowed.\n{}",
        described(&output)
    );
}

#[test]
fn the_verbose_flag_reaches_the_canonical_variable_and_does_not_lift_the_budget() {
    // THE POINT OF THIS TEST. The variable was FLTH_VERBOSE before the
    // wrapper moved to rust-fs-core, and core's script does not read the old
    // name: --verbose would simply stop working, with nothing on stderr and
    // no failing run to notice it. Only a test that looks for the streamed
    // output can tell.
    let output = tier(
        &[
            "verbose",
            "output-budget-test/verbose",
            "5",
            "0",
            "--",
            "bash",
            "-c",
            LOUD,
        ],
        &[("CLI_ARGS", "--verbose")],
    );
    assert_the_wrapper_resolved(&output);

    assert!(
        stdout(&output).contains("line 39"),
        "`--verbose` in CLI_ARGS must reach OUTPUT_BUDGET_VERBOSE and stream \
         the run.\n{}",
        described(&output)
    );
    assert_eq!(
        output.status.code(),
        Some(65),
        "watching a run does not make it cheaper to read later: the budget is \
         enforced either way.\n{}",
        described(&output)
    );
}

#[test]
fn the_resolver_refuses_a_core_that_is_absent() {
    let marker = scratch("absent").join("the-command-ran");
    let command = format!(": > {}", marker.display());
    let output = tier(
        &[
            "absent",
            "output-budget-test/absent",
            "400",
            "0",
            "--",
            "bash",
            "-c",
            &command,
        ],
        &[("FS_CORE_ROOT", &relative_root("absent"))],
    );

    assert_eq!(
        output.status.code(),
        Some(1),
        "a core that is not there is a failure, not a fallback.\n{}",
        described(&output)
    );
    let complaint = stderr(&output);
    for expected in [
        "FS_CORE_ROOT",
        "scripts/output-budget.sh",
        "rust-fs-core-output-budget 1",
        "v0.2.13",
    ] {
        assert!(
            complaint.contains(expected),
            "the refusal must name {expected}, so the reader knows what would \
             provide it.\n{}",
            described(&output)
        );
    }
    assert!(
        !marker.exists(),
        "the tier ran its command without a wrapper it had verified."
    );
}

#[test]
fn the_resolver_refuses_a_core_whose_version_string_is_wrong() {
    // The name it answers with is the one this script used to be: a copy of
    // fs-linux-test-harness's wrapper. That is the copy most likely to still
    // be lying around on a developer's machine, and it is not the contract.
    let (_root, ran) = fake_core("wrong-version", "fs-linux-test-harness-output-budget 1");
    let output = tier(
        &[
            "wrong",
            "output-budget-test/wrong",
            "400",
            "0",
            "--",
            "echo",
            "hush",
        ],
        &[("FS_CORE_ROOT", &relative_root("wrong-version"))],
    );

    assert_eq!(
        output.status.code(),
        Some(1),
        "a present-but-wrong wrapper is fatal.\n{}",
        described(&output)
    );
    let complaint = stderr(&output);
    assert!(
        complaint.contains("fs-linux-test-harness-output-budget 1")
            && complaint.contains("rust-fs-core-output-budget 1"),
        "the refusal must say what it found and what it required.\n{}",
        described(&output)
    );
    assert!(
        !ran.exists(),
        "the wrong wrapper was used anyway -- a version check that does not \
         stop the run checks nothing."
    );
    assert!(
        !log_of("output-budget-test/wrong").exists(),
        "nothing may fall through to another source: finding a second \
         output-budget.sh is how this family ended up with several."
    );
}

#[test]
fn an_override_wins_over_the_sibling_checkout() {
    // FS_CORE_ROOT is how the workflows point at the core checkout that
    // carries the wrapper while the path dependency stays on the pin this
    // crate is held to (#96, #99), and how the refusals above are provoked.
    // An override that loses to a sibling on disk is not an override.
    let (_root, ran) = fake_core("right-version", "rust-fs-core-output-budget 1");
    let output = tier(
        &[
            "override",
            "output-budget-test/override",
            "400",
            "0",
            "--",
            "echo",
            "hush",
        ],
        &[("FS_CORE_ROOT", &relative_root("right-version"))],
    );

    assert!(output.status.success(), "{}", described(&output));
    assert!(
        ran.exists(),
        "the wrapper named by FS_CORE_ROOT was not the one the tier ran.\n{}",
        described(&output)
    );
}

#[test]
fn the_run_copy_is_removed_when_the_tier_ends_badly() {
    // THE ASSERTION IS IN `tier()`, which compares the wrapper copies in
    // tmp/ before and after every run in this file. The case worth its own
    // name is the failing one: that is the exit path the `trap` exists for,
    // and the one an `exec` -- which is what this script used to do -- would
    // have skipped.
    let output = tier(
        &[
            "swept",
            "output-budget-test/swept",
            "400",
            "0",
            "--",
            "bash",
            "-c",
            "exit 9",
        ],
        &[],
    );
    assert_the_wrapper_resolved(&output);
    assert_eq!(output.status.code(), Some(9), "{}", described(&output));
    assert!(
        log_of("output-budget-test/swept").exists(),
        "a failing tier still keeps the whole run in its log.\n{}",
        described(&output)
    );
}

#[test]
fn no_copy_of_the_wrapper_is_committed_here() {
    assert!(
        !repo().join("scripts").join("output-budget.sh").exists(),
        "scripts/output-budget.sh is back. The wrapper is rust-fs-core's and \
         is resolved at run time by scripts/tier.sh; a copy here is a copy \
         that drifts, which is the whole of rust-fs-core#153. If core's \
         script is missing something, change it THERE and raise the pin."
    );
}
