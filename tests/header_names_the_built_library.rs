//! The header names the library the build actually produces.
//!
//! `include/vhd.h` said "Link with libam_img_vhd.a" for as long as the file
//! existed, and cargo has never produced that name: `[lib] name` is
//! `vhd`, so the artefact is `libvhd.a` — which is exactly what
//! `chores.yml` copies. A C consumer following the header got a linker
//! error for a library nobody builds.
//!
//! **The expected name is DERIVED from `Cargo.toml`, not written down
//! here**, so renaming the library fails this test rather than silently
//! making the header wrong again.
//!
//! # Why `toml` and not a hand parse
//!
//! The first version of this file hand-parsed `Cargo.toml` by scanning
//! lines, and rejected three spellings cargo accepts: `name = 'vhd'`
//! in single quotes, `name = "vhd" # comment` with a trailing
//! comment, and `[lib] # comment` — the last making the test claim the
//! manifest declares no library at all. A guard that a legal edit to the file it
//! guards can break is one somebody deletes rather than fixes.
//!
//! A guard asserting something about a structured file must PARSE it.
//! `toml` is a dev-dependency only, so nothing reaches a consumer.
//!
//! The C header is the exception, and deliberately: there is no parser
//! for it here, so `libraries_named` scans. That is a stated limit
//! rather than a quiet one.

use std::path::Path;

fn read(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// `[lib] name`, parsed.
fn lib_name(cargo_toml: &str) -> Option<String> {
    let doc: toml::Value = toml::from_str(cargo_toml).ok()?;
    Some(doc.get("lib")?.get("name")?.as_str()?.to_owned())
}

/// Every `lib<something>.a` the header mentions. A scan, because a C
/// header has no parser here — see the module note.
fn libraries_named(header: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in header.lines() {
        let mut rest = line;
        while let Some(i) = rest.find("lib") {
            let tail = &rest[i..];
            let name: String = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            if name.ends_with(".a") && !out.contains(&name) {
                out.push(name);
            }
            rest = &rest[i + 3..];
        }
    }
    out
}

#[test]
fn the_header_tells_consumers_to_link_the_library_that_is_built() {
    let cargo = read("Cargo.toml");
    let name = lib_name(&cargo).expect("Cargo.toml declares [lib] name");
    let want = format!("lib{name}.a");

    // Named explicitly: a missing header here almost always means the
    // manifest and the shipped files have drifted apart, and "No such
    // file" on its own does not say so.
    let header_path = format!("include/{name}.h");
    let full = Path::new(env!("CARGO_MANIFEST_DIR")).join(&header_path);
    assert!(
        full.exists(),
        "Cargo.toml declares [lib] name {name:?}, so the build produces {want} and \
         the C header for it should be {header_path} -- which does not exist. The \
         manifest and the shipped headers have drifted apart."
    );
    let header = read(&header_path);
    let named = libraries_named(&header);

    // Asserted before it is compared: a scan that found nothing would
    // make the loop below pass over an empty list, which is the shape
    // of defect this file exists for.
    assert!(
        !named.is_empty(),
        "include/{name}.h names no lib*.a at all, so it gives a C consumer no link \
         guidance. It should name {want}."
    );

    for got in &named {
        assert_eq!(
            got, &want,
            "include/{name}.h tells consumers to link {got}, but Cargo.toml's \
             [lib] name is {name:?}, so the build produces {want}. Linking {got} \
             fails: nothing builds it."
        );
    }
}

/// THE SPELLINGS A HAND PARSE GOT WRONG.
///
/// Every one of these is valid TOML that cargo accepts, and every one
/// of them broke the previous version of this file — two by reading a
/// mangled name, one by concluding there was no `[lib]` section. They
/// are here as acceptance cases rather than rejection cases, because
/// the fix has to be shown to ACCEPT what it used to refuse; a parser
/// that merely still handles the plain spelling proves nothing.
#[test]
fn the_lib_name_is_parsed_rather_than_scanned() {
    let plain = "[package]\nname = \"am-img-vhd\"\n\n[lib]\nname = \"vhd\"\n";
    let single_quoted = "[package]\nname = \"am-img-vhd\"\n\n[lib]\nname = 'vhd'\n";
    let trailing_comment =
        "[package]\nname = \"am-img-vhd\"\n\n[lib]\nname = \"vhd\" # the exported ABI name\n";
    let commented_section =
        "[package]\nname = \"am-img-vhd\"\n\n[lib] # the staticlib consumers link\nname = \"vhd\"\n";

    for (what, toml) in [
        ("the plain spelling", plain),
        ("a single-quoted string", single_quoted),
        ("a trailing comment", trailing_comment),
        ("a comment on the section header", commented_section),
    ] {
        assert_eq!(
            lib_name(toml).as_deref(),
            Some("vhd"),
            "{what} is valid TOML and cargo accepts it, so this guard must too"
        );
    }
}

/// The package name is not the library name.
#[test]
fn the_lib_name_comes_from_the_lib_section_and_not_the_package() {
    let toml = "[package]\nname = \"am-img-vhd\"\nversion = \"0.3.5\"\n\n\
                [lib]\nname = \"vhd\"\ncrate-type = [\"staticlib\", \"rlib\"]\n";
    assert_eq!(
        lib_name(toml).as_deref(),
        Some("vhd"),
        "using the package name would look for libam-img-vhd.a"
    );
    // And a manifest with no [lib] section has no library name to give.
    assert_eq!(lib_name("[package]\nname = \"am-img-vhd\"\n"), None);
}

/// The header scan finds a library name wherever it sits in a line.
#[test]
fn the_header_scan_finds_library_names_in_prose() {
    let header = " * Link with libvhd.a and include this header alongside fs_core.h.\n";
    assert_eq!(libraries_named(header), vec!["libvhd.a"]);

    // The exact defect, and the near-miss that has to stay distinct.
    assert_eq!(
        libraries_named(" * Link with libam_img_vhd.a and include this\n"),
        vec!["libam_img_vhd.a"]
    );

    // Words merely beginning with "lib" are not libraries.
    assert!(libraries_named(" * This library is liberally licensed.\n").is_empty());
}

/// `vars.LIBNAME` out of `chores.yml`, parsed.
///
/// YAML, so it is parsed rather than scanned — `saphyr` is the adopted
/// parser for it, the way `toml` is for the manifest.
fn chores_libname(chores_yml: &str) -> Option<String> {
    use saphyr::{LoadableYamlNode, Yaml};
    let docs = Yaml::load_from_str(chores_yml).ok()?;
    let doc = docs.first()?;
    // as_mapping_get, not indexing: saphyr's Index PANICS on a missing
    // key, so a chores file without the variable would abort the test
    // rather than report its absence -- and reporting absence is half
    // of what this function is for.
    doc.as_mapping_get("vars")?
        .as_mapping_get("LIBNAME")?
        .as_str()
        .map(|s| s.to_owned())
}

/// THE PACKAGING VARIABLE AGREES WITH THE MANIFEST.
///
/// `chores.yml` copies `target/<triple>/release/lib{{.LIBNAME}}.a`, but
/// what cargo builds is named by `[lib] name`. Nothing tied the two
/// together: the header check compares against the manifest, and
/// `LIBNAME` was free to drift from it independently.
///
/// A drift did fail — but LATE and unrecognisably, after a full release
/// cross-compile, as `cp: cannot stat .../libNAME.a`. The guard runs
/// first precisely so a naming mistake costs no build, and this was the
/// one naming mistake it did not cover.
#[test]
fn the_packaging_variable_matches_the_manifest() {
    let name = lib_name(&read("Cargo.toml")).expect("Cargo.toml declares [lib] name");
    let libname = chores_libname(&read("chores.yml")).expect("chores.yml declares vars.LIBNAME");
    assert_eq!(
        libname, name,
        "chores.yml sets LIBNAME={libname:?} and Cargo.toml sets [lib] name={name:?}. \
         cargo builds lib{name}.a, chores copies lib{libname}.a, and the packaging step \
         fails with `cp: cannot stat` after the release build rather than here."
    );
}

/// The chores parse reads YAML rather than matching a line.
#[test]
fn the_libname_is_parsed_rather_than_scanned() {
    for (what, yml) in [
        ("a plain value", "vars:\n  LIBNAME: vhd\n"),
        ("a quoted value", "vars:\n  LIBNAME: \"vhd\"\n"),
        (
            "a trailing comment",
            "vars:\n  LIBNAME: vhd # the linked name\n",
        ),
        (
            "the key named in a comment first",
            "# LIBNAME: wrong\nvars:\n  LIBNAME: vhd\n",
        ),
    ] {
        assert_eq!(
            chores_libname(yml).as_deref(),
            Some("vhd"),
            "{what} is valid YAML, so this guard must read it"
        );
    }
    assert_eq!(
        chores_libname("tasks:\n  build:\n    cmds: ['cargo build']\n"),
        None,
        "a chores file with no LIBNAME has none to report"
    );
}

// ---------------------------------------------------------------------
// `sources:` names what the task reads
// ---------------------------------------------------------------------

/// A `staticlib` field, as a list of scalars. `sources` and `cmds` are
/// both sequences of strings, so one reader serves both.
///
/// Parsed, not scanned, for the reason `chores_libname` is: the entries
/// are quoted or bare at the author's discretion, comments sit between
/// them, and a reader that matched lines would disagree with the tool
/// that actually runs the task.
fn staticlib_list(chores_yml: &str, field: &str) -> Vec<String> {
    use saphyr::{LoadableYamlNode, Yaml};
    let Ok(docs) = Yaml::load_from_str(chores_yml) else {
        return Vec::new();
    };
    let Some(doc) = docs.first() else {
        return Vec::new();
    };
    doc.as_mapping_get("tasks")
        .and_then(|t| t.as_mapping_get("staticlib"))
        .and_then(|t| t.as_mapping_get(field))
        .and_then(Yaml::as_sequence)
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The test targets the task actually runs: the argument after each
/// `--test` in its command list.
///
/// By token rather than by regex, because `cargo test --locked --test x`
/// and `cargo test --test x --locked` are the same command and a
/// pattern anchored to the first spelling would read the second as
/// running nothing — which is the empty answer this check must never
/// mistake for a clean one.
fn test_targets_run(chores_yml: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for cmd in staticlib_list(chores_yml, "cmds") {
        let mut words = cmd.split_whitespace();
        while let Some(word) = words.next() {
            if word == "--test" {
                if let Some(name) = words.next() {
                    targets.push(name.to_owned());
                }
            } else if let Some(name) = word.strip_prefix("--test=") {
                targets.push(name.to_owned());
            }
        }
    }
    targets
}

/// The `tests/` entries of `sources:`, whatever quoting they carry.
fn test_sources(chores_yml: &str) -> Vec<String> {
    staticlib_list(chores_yml, "sources")
        .into_iter()
        .filter(|s| s.starts_with("tests/"))
        .collect()
}

/// SOURCES NAMES WHAT THE TASK READS, IN BOTH DIRECTIONS.
///
/// Both directions are real, they pull opposite ways, and fixing one is
/// how you get the other — which is why one check refuses both rather
/// than two checks each refusing one.
///
/// TOO NARROW is what this repository shipped, and `rust-img-vhdx#79`
/// measured it: with no `tests/` entry at all, editing the guard left
/// the task up to date and the guard did not run. Force its body to
/// `false`, change nothing else, and `chore staticlib` prints
/// "task: staticlib is up to date" and executes nothing. A check in the
/// step that decides what ships, whose result nothing re-reads.
///
/// TOO WIDE is `rust-img-vhdx#85`, and it is the trap inside the
/// obvious remedy: vhdx closed the narrow case with `tests/**/*.rs`,
/// which fingerprints every file in the directory while `cmds:` runs
/// one target, so editing an unrelated test re-runs the cross-target
/// release build. It cannot ship a stale artefact — it errs toward
/// rebuilding — so it costs time and nothing else, which is exactly why
/// it survived a review. A verbatim copy from vhdx reproduces it here.
#[test]
fn the_staticlib_task_fingerprints_the_tests_it_runs_and_no_others() {
    let chores = read("chores.yml");
    let run = test_targets_run(&chores);
    let sources = test_sources(&chores);

    // The task runs a test at all. Without this the two comparisons
    // below are between empty lists and agree vacuously -- which is what
    // a `cmds:` block that lost its `cargo test` line would look like.
    assert!(
        !run.is_empty(),
        "the staticlib task runs no `--test` target at all. The header guard is that \
         target, and it runs FIRST so a naming mistake costs no release build. Read \
         cmds: {:?}",
        staticlib_list(&chores, "cmds")
    );

    // A GLOB IS THE TOO-WIDE CASE BY CONSTRUCTION, and it is checked
    // first so it is reported as itself. It matches whatever the
    // directory happens to hold, so it does cover the guard -- which
    // means the "is the guard fingerprinted" check below would pass
    // over it, and the failure would be reported as #79 when it is #85.
    // A message that names the wrong defect is worse here than no
    // message: this file is copied into four sibling repositories.
    for have in &sources {
        assert!(
            !have.contains('*'),
            "chores.yml fingerprints {have} under staticlib's sources:. That is a \
             glob over a directory, and the task opens one file in it, so editing an \
             unrelated test re-runs the whole task including the cross-target release \
             build. That is #85. Name the file instead: the task runs {run:?}"
        );
    }

    let wanted: Vec<String> = run.iter().map(|t| format!("tests/{t}.rs")).collect();
    for want in &wanted {
        assert!(
            sources.contains(want),
            "chores.yml runs {want} in staticlib but does not list it under sources:, \
             so editing it leaves the task up to date and the guard does not run. \
             That is #79. sources: names {sources:?}"
        );
    }
    for have in &sources {
        assert!(
            wanted.contains(have),
            "chores.yml fingerprints {have} under staticlib's sources: and the task \
             never opens it, so editing an unrelated test re-runs the cross-target \
             release build. That is #85. The task runs {run:?}"
        );
    }
}

/// The too-wide direction, on a manifest rather than on the tree: the
/// glob `rust-img-vhdx` shipped, which is what copying its fix verbatim
/// produces here.
#[test]
fn a_tests_glob_fingerprints_files_the_task_never_opens() {
    let chores = concat!(
        "tasks:\n  staticlib:\n",
        "    sources:\n      - 'tests/**/*.rs'\n",
        "    cmds:\n      - 'cargo test --locked --test header_names_the_built_library'\n",
    );
    assert_eq!(
        test_targets_run(chores),
        vec!["header_names_the_built_library"],
        "one target is run"
    );
    let sources = test_sources(chores);
    assert_eq!(sources, vec!["tests/**/*.rs"], "the glob is read as itself");
    assert!(
        sources[0].contains('*'),
        "and it is recognised AS a glob, which is what makes the failure report #85 \
         rather than #79 -- a glob does cover the guard's own file, so the \
         is-the-guard-fingerprinted check would pass over it"
    );
}

/// The too-narrow direction: no `tests/` entry, so nothing pins the
/// guard's own file.
#[test]
fn a_sources_list_with_no_test_file_pins_the_guard_to_nothing() {
    let chores = concat!(
        "tasks:\n  staticlib:\n",
        "    sources:\n      - Cargo.toml\n      - chores.yml\n",
        "    cmds:\n      - 'cargo test --locked --test header_names_the_built_library'\n",
    );
    assert!(
        test_sources(chores).is_empty(),
        "nothing under tests/ is fingerprinted"
    );
    assert_eq!(
        test_targets_run(chores),
        vec!["header_names_the_built_library"]
    );
}

/// The acceptance half, and the one that stops the reader being the
/// defect: a command list this check must read correctly however the
/// flags are ordered or spelled, and a second target being legal.
#[test]
fn the_target_is_read_from_the_command_however_it_is_spelled() {
    for (what, cmds) in [
        (
            "the flag last",
            "      - 'cargo test --locked --test header_names_the_built_library'\n",
        ),
        (
            "the flag first",
            "      - 'cargo test --test header_names_the_built_library --locked'\n",
        ),
        (
            "an equals sign",
            "      - 'cargo test --locked --test=header_names_the_built_library'\n",
        ),
    ] {
        let chores = format!("tasks:\n  staticlib:\n    cmds:\n{cmds}");
        assert_eq!(
            test_targets_run(&chores),
            vec!["header_names_the_built_library"],
            "{what} is the same command and cargo runs it either way"
        );
    }

    // Two targets is a legal task, and then both files belong in
    // sources:. The check is a correspondence, not a count.
    let chores = concat!(
        "tasks:\n  staticlib:\n",
        "    sources:\n      - 'tests/one.rs'\n      - 'tests/two.rs'\n",
        "    cmds:\n      - 'cargo test --test one'\n      - 'cargo test --test two'\n",
    );
    let run = test_targets_run(chores);
    assert_eq!(run, vec!["one", "two"]);
    assert_eq!(test_sources(chores), vec!["tests/one.rs", "tests/two.rs"]);

    // A command that runs no test contributes no target, rather than
    // contributing an empty one.
    let chores = "tasks:\n  staticlib:\n    cmds:\n      - 'cargo build --release'\n      - 'rustup target add x'\n";
    assert!(test_targets_run(chores).is_empty());
}
