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

<!-- BEGIN SHARED BLOCK: agent-core v1 sha256:60fad6dd98e9da3e9256d38728b02ac189dca0d04fc98c13e2c67de3f3103319 -->
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
that log, and a failure prints its tail. CI keeps the logs as an artifact, so
the detail is always retrievable.

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

## Project rules

- **No GPL/LGPL/AGPL dependencies.** Permissive only (MIT/BSD/Apache).
  Shelling out to a copyleft CLI as a *test oracle* is fine — linking or
  copying it is not.
- **Each of these is a standalone project.** Never mention a consuming
  application in the README, the source, or CLI help.
<!-- END SHARED BLOCK: agent-core v1 -->
## What this is

Pure-Rust VHD reader and writer over `am-fs-core`, covering fixed, dynamic and
differencing images, linked into the app as a staticlib.

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

## The pin you cannot bump, and why

This crate depends on `am-fs-core` and is **pinned to `v0.2.10`**, one release
behind, and that is deliberate.

`4e19fc9` (rust-fs-core#75) made a write past the end of a `FileDevice` a
refusal rather than an implicit extension. It was right to — `size_bytes()`
reported the construction-time length while the file grew underneath it, so
`CachingDevice` could serve bytes no cached read could reach (#70). But writing
past the end was **the only way this format allocates**: append a block, cluster
or grain, then record where it went.

Measured against core `main`: vhd 7 failures, qcow2 3, vhdx 1, vmdk 1; zero
against `v0.2.10`. Every one is a write landing exactly at the device's current
end.

Do **not** bump the pin, and do **not** "fix" it by reverting #75 — that
reintroduces #70. Tracked as rust-fs-core#147/#129; the agreed replacement is
`BlockDevice::set_len` plus `can_grow()`.

One practical consequence: `pre-commit.d/rust-clippy.sh` runs clippy without
`--locked`, so a `../rust-fs-core` checkout that is semver-ahead of the pin
rewrites your unstaged `Cargo.lock`, and `rust-deps-pinned.sh` then blocks the
commit over a file the commit never contained. That is a livelock
(agent-skills#64). Work from a throwaway worktree with `../rust-fs-core` at
`v0.2.10` rather than reaching for `--no-verify`, which disables every guard at
once.

## What gates a merge

One required check, `ci-ok`, declared in `.github-guard` and aggregating every
job in `ci.yml`. `fuzz.yml` (nightly cron plus dispatch) and `release.yml`
(tag-driven) never report on a pull request and must never be required.

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
