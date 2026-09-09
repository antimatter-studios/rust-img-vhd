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
