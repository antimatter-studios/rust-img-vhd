# Working in rust-img-vhd (agent guide)

Pure-Rust VHD reader and writer — fixed, dynamic and differencing — validated against `qemu-img`. This file is the fast path
for an agent picking up work here, so the workflow does not have to be
re-derived each time. It points at the existing docs rather than duplicating
them:

- **README** → what the crate does, how it is built, and what does not work yet.
- **`chores.yml`** → every task named below, and what each one actually runs.
- **`.github-guard`** → what must pass before `main` takes a merge.

The section between the BEGIN/END markers below is **shared, byte-identical,
with every repository in this family**. Do not edit it here: change the
canonical copy and propagate it, or `scripts/agents-core-check.sh` will fail.
Everything after the END marker is specific to this repository.

<!-- BEGIN SHARED BLOCK: agent-core v7 sha256:1d6c04c827b22c2836e4b9d3977fda083f1ec322d8408a6b2ae5bf72de42b3e5 -->
## Claiming work

Several agents work these repositories at the same time. Before you start on
an issue, claim it, so nobody else spends a session on what you are already
doing. The lock is a **GitHub label**, because labels are shared state that
every agent can read and change without posting comments into the thread.

**Before starting.** Check, claim, then read back:

```sh
gh issue view <N> --json labels                      # holds `claimed`? pick another
gh issue edit <N> --add-label claimed --add-label claim/<session>
gh issue view <N> --json labels                      # read back and confirm
```

`<session>` is your session name — `agent-<random4>-<isodate>`, e.g.
`agent-3f7c-2026-09-22`. Create the `claim/<session>` label if it does not
exist.

**Resolving a race.** Adding a label is not compare-and-swap: two agents can
both add `claimed` and both believe they won. That is what the read-back is
for. If it shows more than one `claim/*` label, the **lexically lowest**
session keeps the issue; every other agent removes its own `claim/*` label and
picks different work. Each racer computes the same answer independently, so no
further coordination is needed.

**When you finish or stop.** Remove both labels — on merge, or the moment you
abandon the work:

```sh
gh issue edit <N> --remove-label claimed --remove-label claim/<session>
```

Delete your `claim/<session>` label from the repository at the end of your
session so they do not accumulate.

**Reclaiming a stale claim.** An agent that dies holding a claim would block an
issue forever. If `claimed` was applied more than 12 hours ago and the holder's
branch has no commits since, any agent may take it: remove the stale `claim/*`,
add your own, and say so in the issue.

**This is a convention, not a fence.** Nothing enforces it. An agent that
ignores it duplicates work; it cannot corrupt anything. Honour it anyway.

## Findings become issues before a tracking issue closes

A review, an investigation, or the work under an umbrella issue turns up
things beyond what it set out to do. File each finding as its own issue --
what was seen, where, and what would show it fixed -- and cite it in the
tracking issue **before** that issue closes. A finding left in a comment, a
pull request description or a transcript is lost when the issue that held
it closes; an open issue is where the next agent looks.

## Work in a worktree

Every working copy is a **git worktree** of an existing checkout, made with
`git worktree add`. Never `git clone` a second, unlinked copy — not for a
branch, a PR, a review, or a sibling you need at another ref:

```sh
git -C <checkout> fetch origin
git -C <checkout> worktree add <path> -b <type>/<name> origin/main   # new work
git -C <checkout> worktree add --detach <path> <tag>                 # a sibling at a pinned ref
git -C <checkout> worktree remove <path>                             # when done
```

A worktree shares the checkout's objects and remotes, and `git worktree list`
shows it to every agent on the machine, so nobody else mistakes it for
abandoned work or loses track of it. An unlinked clone copies all the history
again, is invisible to that list, and gets left behind in `/tmp` long after the
work that made it is merged. Remove your worktree when you finish.

## Skills to use

- **`dev-loop`** — the required loop for any non-trivial change: baseline the
  full suite → change → re-run (no baseline test may regress) → enhance tests →
  vet. Always run it.
- **`commit`** / **`pr`** — for grouping commits and opening pull requests.

Each repository names any further skills of its own below.

## A bug fix starts with a red

**Prove it is broken first** — a failing check or test — *then* fix it, *then*
prove that same check is green, *then* confirm the full baseline still passes.
Never write the fix before you have a red. A fix with no failing test to its
name is a claim, not a result.

**Red and green happen in one pull request, on one branch.** Push the commit
that adds the failing test on its own, and let CI show it red on that pull
request. Then push the fix to the **same branch**, with the test untouched,
until the same pull request is green.

- Never put a fix in a second pull request, stacked or not. A pull request
  that only holds a red commit can never merge, and it blocks every pull
  request built on it.
- Never push the test and the fix together. The red has to be visible in CI,
  not claimed in the description.
- Wait for the red run to finish before pushing the fix: a new push cancels
  the run in progress.

## Nothing skips

A test that cannot run **fails**, naming the task that would provide what it
needed. Never add an early return for a missing fixture, tool or VM: a skipped
test reads exactly like a passing one, and a suite that quietly declines to run
is indistinguishable from a suite that passes.

Where a tier reports skips or ignored tests, that is a gate, not a note.

## Validate against something that is not us

A driver's own readers share its interpretation of the format, so they cannot
catch a misreading: the mistake is baked into the fixture *and* the parser, and
they agree with each other while disagreeing with every real filesystem. Unit
tests over self-built fixtures prove self-consistency, not correctness.

Every structure that is parsed or written gets a cross-validation test against
an **independent oracle** — the platform's own tools, a real kernel, or a third
implementation — before it is considered done. Each repository names its
oracles below.

## Output is budgeted

Test tiers run through `scripts/tier.sh`, which runs the suite **quietly**: the
whole run goes to `tmp/logs/<tier>.log`, a pass prints one verdict line naming
that log, and a failure prints the verdict, the command's status and the log's
path — `--tail N`, or `OUTPUT_BUDGET_FAIL_TAIL=N`, prints the tail for whoever
is watching. **Read the log**: a failing tier names it and does not recite it.
CI keeps the logs as an artifact, so the detail is always retrievable.

The budget caps the log, not merely what is shown, and every number in the
table was measured. A run that passes but prints more than its budget **fails**.

The reader who pays most for a noisy suite is an agent that re-reads its whole
transcript on every step, and so pays for one loud run many times over. If a
tier legitimately grows, raise its row **with the measurement that justifies
it**. Do not silence output to fit, and do not route around `tier.sh`.

## Commits and branches

- Branches are `<type>/<name>`, matching the commit type: `fix/`, `feat/`,
  `ci/`, `docs/`, `chore/`, `test/`.
- A commit is a subject plus flat one-sentence bullets. Subjects are
  declarative, not imperative: "the run-end bound is checked", not "check the
  run-end bound".
- **No AI attribution and no co-author trailers**, in commits or in pull
  request descriptions.
- `main` takes **squash merges only**.
- **Bring a branch up to date before every push.** `git fetch origin`, and
  if its base has moved -- the layer below it in the stack, or `main` at the
  bottom -- rebase onto the base first, resolving any conflicts then, while
  they are small. Synced first, the run
  tests the branch close to how it will land. And a branch synced at every
  push never goes stale: each sync takes in only what landed since the last
  one, so conflicts stay few and small. Sync when you are pushing anyway; a
  push made only to bring a branch up to date buys a whole CI run and
  nothing else.
- **`main` merges through a merge queue.** A stack whose pull requests'
  own CI is green goes into the queue: `gh stack merge <stack> --yes` adds
  every layer up to the one named. Never queue a stacked pull request with
  the REST `merge-async` call: it answers "enqueued" and queues nothing. The
  queue tests each group on a temporary branch, `main` plus the queued pull
  requests in order, and squash-merges each one only when that run is green,
  so what lands is exactly the tree CI ran. That run is what "branches must be up to
  date" used to buy, at one run per group instead of one per pull request per
  merge: never update a branch only so that it can merge. Every workflow
  that gates `main` triggers on `merge_group`, or the queue never sees its
  result and nothing merges.

## One stack per repository

Every repository lands its pull requests as one stack, whether or not they
touch the same files. Conflicts are then paid for once, when a pull request
joins the stack, instead of again in the merge queue after every merge.

- **A new pull request goes on the top.** Ask
  `../rust-fs-core/scripts/stack-base.sh` (run in place; in rust-fs-core
  itself, `scripts/stack-base.sh`): it prints the head branch of the top of
  the stack, or `main` when nothing is open. Rebase onto that branch, resolve
  every conflict locally while you do, and build and lint the result before
  you push. A merge that is clean on your machine is clean in CI, so a
  conflict that reaches the queue means the base was stale or the technique
  was wrong. Then link the pull request into the stack: `gh stack link
  <stack> <pull request>`.
- **One squashed commit per layer.** Squash the branch to one commit on the
  layer below before it joins, so the layers above replay without resolving
  anything again.
- **The queue still tests every layer.** It runs the full pipeline on `main`
  plus each layer and the layers below it, and squash-merges each layer as
  its own commit, so nothing lands untested and `main` stays bisectable.
- **A red layer stops the stack.** Fix it in place, on its own branch, then
  replay every layer above it with `git rebase --update-refs` and push them
  together. Do not move a red layer to the top: the layers above were
  resolved against it. Take a layer out only when its fix cannot come soon,
  and say so on its pull request.
- **Never delete a branch a pull request is based on.** Deleting it closes
  every pull request above it, and none can be reopened while it is gone.

## Changelog entries are fragments

A pull request never edits `CHANGELOG.md`. It adds one file under
`changelog.d/`, named for its branch (`fix/run-end-bound` writes
`changelog.d/fix-run-end-bound.md`), holding one or more `### Added`,
`### Changed`, `### Fixed` (or any other) headings, each over `- ` entries.
Pull requests open at the same time then never conflict over the top of one
file. A change no consumer would notice -- tests, CI, documentation -- adds
no fragment.

A release gathers them: `../rust-fs-core/scripts/changelog-gather.sh
VERSION` writes every fragment into a dated `## [VERSION]` section of
`CHANGELOG.md` and deletes the fragments, in the release commit. An entry is
a fragment until it ships and a line of `CHANGELOG.md` after, never both.
`chore lint` runs `changelog-check.sh`: every fragment is well formed, and
`CHANGELOG.md` holds no unreleased entries of its own.

## Project rules

- **No GPL/LGPL/AGPL dependencies.** Permissive only (MIT/BSD/Apache).
  Shelling out to a copyleft CLI as a *test oracle* is fine — linking or
  copying it is not.
- **Each of these is a standalone project.** Never mention a consuming
  application in the README, the source, or CLI help.
<!-- END SHARED BLOCK: agent-core v7 -->
## What this is

Pure-Rust VHD reader and writer over `rust-fs-core`, covering fixed, dynamic and
differencing images, linked into the app as a staticlib.

## Keep the features page current

`docs/features.md` lists every feature with its state, the release it shipped
in, its tracking issue and the test that checks it. **A pull request that
adds, fixes, refuses or removes behaviour updates its row in the same pull
request**, with the PR number under **Since** (`Unreleased (#N)` until a
release, then the version). A finding that leaves something unsupported gets
a row too, naming its issue. Releases turn every `Unreleased` into the
version they ship as.

## Running tests

```sh
chore test          # the suite
chore testqemu      # against qemu-img
chore testrelease   # release profile
chore lint          # fmt, the agent-core check, clippy
chore staticlib     # what the app links
```

CI runs `test`, `test-release`, `qemu-validation`, `fmt`, aggregated by `ci-ok`.

## The oracle is qemu-img

An image this crate writes must be one `qemu-img` reads identically, and one
`qemu-img` wrote must read identically here. Our own reader shares our writer's
interpretation, so only a third implementation catches a misreading they agree
on. If `qemu-img` is missing the job **fails**; it does not skip.

Dynamic and differencing blocks are allocated **at the file tail**, which is
exactly the operation the core pin below is about.

Both profiles are run deliberately: arithmetic that panics in debug can wrap
silently in release. `tests/ci_profile.rs` holds the debug run to being one.

## The pin, and why it sat still

This crate depends on `rust-fs-core` at **`v0.2.12`**. It was held at `v0.2.10`
for a while, and the reason still shapes the write path.

`4e19fc9` (rust-fs-core#75) made a write past the end of a `FileDevice` a
refusal rather than an implicit extension. It was right to — `size_bytes()`
reported the construction-time length while the file grew underneath it, so
`CachingDevice` could serve bytes no cached read could reach (#70). But writing
past the end was **the only way this format allocates**: append a block, then
record where it went. Against `v0.2.11` and `v0.2.13` that cost 7 failures in
`tests/synthetic.rs`, every one a write landing exactly at the device's end.

The fix is `BlockDevice::set_len` plus `can_grow()`, first released in core
`v0.2.12` (rust-fs-core#147/#129; #96 and #99 here).
`VhdReader::allocate_block_locked` grows the device to the end of the
footer's new copy **before** it writes anything, so a device that cannot grow
refuses the allocation with the image untouched. Do **not** "fix" a refused
write by reverting #75 — that reintroduces #70. A test double that wraps a
device must forward `set_len` and `can_grow`: the trait defaults answer
`Err(ReadOnly)` / `false`, and it compiles.

## rust-fs-core comes from crates.io, not the sibling

`Cargo.toml` and `fuzz/Cargo.toml` take `rust-fs-core` from the registry,
at the version `Cargo.lock` records (#148). It was a `path` dependency on
`../rust-fs-core` until 0.6.0, and that made every build from a checkout,
release tags included, fail without a core checkout beside it. Windows has no
release tarball, so building from a checkout is the only way to get the tool
there, and that build failed too. The `standalone` CI job checks this
repository out alone on Linux and Windows and builds the tool, so a path
dependency on a sibling cannot come back unnoticed.

The `../rust-fs-core` checkout is still needed, for core's scripts (the
tiers, the floors, packaging, semver) and for the `fs_core.h` that
`chore staticlib` ships. Keep it at the tag the workflows pin, which is the
version `Cargo.lock` resolves.

To build against an unreleased core, override it on your machine only, in a
`.cargo/config.toml` you do not commit:

```toml
[patch.crates-io]
rust-fs-core = { path = "../rust-fs-core" }
```

Bump the version here once that core is published.

## The output budget comes from rust-fs-core

Every tier runs through rust-fs-core's `scripts/tier.sh`, **run in place**
from the `../rust-fs-core` checkout at the version this repository pins:
`bash ../rust-fs-core/scripts/tier.sh LABEL LOG LINES BYTES -- COMMAND`.
There is no copy of it, of `scripts/output-budget.sh`, or of any other family
script in this repository, and rust-fs-core's `family-check` (run in CI)
refuses one. Bumping the pinned core version is how the scripts are upgraded;
nothing is recopied. See rust-fs-core#153 and #212.

**The verbose variable is `OUTPUT_BUDGET_VERBOSE`.** It was `FLTH_VERBOSE`
while the script was vendored, and core's does not read the old name: setting
it does nothing, quietly. `chore test -- --verbose` maps the flag onto the
right one.

## What gates a merge

One required check, `ci-ok`, declared in `.github-guard` and aggregating every
job in `ci.yml`. `fuzz.yml` (nightly cron plus dispatch) and `release.yml`
(tag-driven) never report on a pull request and must never be required.

`chore check:ci-gate` holds both halves of that mechanically — every job in
`ci.yml` must appear in `ci-ok`'s `needs:`, and `.github-guard` must require
`ci-ok` and nothing else. The task runs `scripts/core.sh ci-gate` and nothing else,
so the script is what can be tested, reviewed and run without `chore` at all.
It replaced `tests/ci_aggregate_gate.rs`: that parsed a YAML file and compared
strings, exercising nothing this crate ships, and as a `cargo test` it counted
towards the executed-test floor the gate itself enforces.

Judging mergeability from check **conclusions** is unreliable: an in-progress
`CheckRun` reports its conclusion as an empty string, and a `StatusContext` has
no conclusion field at all. Read `mergeStateStatus` and
`statusCheckRollup.state`.

## Never grow a shared tool to solve a problem here

**Never grow a shared tool to solve a problem in this repository.** `chore` is
a general-purpose task runner this project merely consumes; the same goes for
`github-guard` and the agent-skills hooks. If something needed here looks like
it belongs inside one of them, it does not. Solve it here, or ask first. The
tell is a release: if a shared tool needs a new version cut whose only purpose
is to unblock this project, the code is in the wrong repository.
