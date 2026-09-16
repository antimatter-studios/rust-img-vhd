//! Shared test fixtures.
//!
//! Only the temp-path type lives here so far. The *byte-level* fixtures
//! deliberately stay in each test file, writing literal offsets rather
//! than importing `vhd::format` — see that module's docs for why the
//! second opinion is worth the repetition.

#![allow(dead_code)] // each test binary uses a subset

use std::path::{Path, PathBuf};

/// A temp-file path that removes its file on drop.
///
/// Written three times before this — twice as an RAII type and once, in
/// `synthetic.rs`, as a bare `PathBuf` with a `remove_file` call at the
/// end of each test. That third one leaked: a test that panics on an
/// assertion never reaches its last line, so every failing run left a
/// fixture behind in the temp directory. On CI that is invisible; on a
/// developer's machine it accumulates, and the images here are up to a
/// few MiB each.
///
/// `Drop` runs during unwinding, so this cleans up on the failing path
/// too — which is the only path that ever leaked.
pub struct TempPath(PathBuf);

impl std::ops::Deref for TempPath {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A unique temp path: `<prefix>_<pid>_<n>_<name>.<ext>`.
///
/// The pid keeps two concurrent `cargo test` runs apart; the counter
/// keeps two tests in the same binary apart. Both are needed — cargo
/// runs test binaries in parallel *and* runs the tests inside one
/// binary on several threads.
pub fn tmp_path_with(prefix: &str, name: &str, ext: &str) -> TempPath {
    unique_path_in(std::env::temp_dir(), prefix, name, ext)
}

/// The same unique, self-removing path, in the process's working
/// directory instead of the temp directory.
///
/// For the one kind of test whose point is that a file sits somewhere
/// `tmp_path_with` is not. A fixed name there is shared by every
/// concurrent `cargo test` in the checkout, so one run's cleanup can
/// delete the other's fixture mid-test; and a bare `remove_file` at the
/// end of the test is skipped by a panic before it, leaving the file in
/// the repository.
pub fn cwd_path_with(prefix: &str, name: &str, ext: &str) -> TempPath {
    unique_path_in(
        std::env::current_dir().expect("the working directory is readable"),
        prefix,
        name,
        ext,
    )
}

fn unique_path_in(mut p: PathBuf, prefix: &str, name: &str, ext: &str) -> TempPath {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    p.push(format!("{prefix}_{}_{n}_{name}.{ext}", std::process::id()));
    TempPath(p)
}
