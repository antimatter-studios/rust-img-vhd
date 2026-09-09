//! The header names the library the build actually produces.
//!
//! `include/vhd.h` said "Link with libam_img_vhd.a" for as long as the file
//! existed, and cargo has never produced that name: `[lib] name` is
//! `vhd`, so the artefact is `libvhd.a` — which is exactly what
//! `chores.yml` copies. A C consumer following the header got a linker
//! error for a library nobody builds.
//!
//! Six of the twelve constellation repositories had the same mismatch
//! while five had it right and one named no library at all, so it is
//! not a naming rule anybody applies once — it drifts, and therefore
//! wants a check.
//!
//! **The expected name is DERIVED, not written down here.** It comes
//! from `Cargo.toml`'s `[lib] name`, which is what decides the artefact
//! name, so renaming the library fails this test instead of silently
//! making the header wrong again.

use std::path::Path;

fn read(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// `[lib] name` out of `Cargo.toml`, by hand rather than by acquiring a
/// toml dependency for a test.
fn lib_name(cargo_toml: &str) -> Option<String> {
    let mut in_lib = false;
    for line in cargo_toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_lib = t == "[lib]";
            continue;
        }
        if !in_lib {
            continue;
        }
        if let Some(rest) = t.strip_prefix("name") {
            let rest = rest.trim_start().strip_prefix('=')?.trim();
            return Some(rest.trim_matches('"').to_owned());
        }
    }
    None
}

/// Every `lib<something>.a` the header mentions.
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

    let header = read(&format!("include/{name}.h"));
    let named = libraries_named(&header);

    // Asserted before it is compared: a parse that found nothing would
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

/// The `[lib] name` parse reads the right section.
#[test]
fn the_lib_name_comes_from_the_lib_section_and_not_the_package() {
    let toml = "\
[package]
name = \"am-img-vhd\"
version = \"0.3.5\"

[lib]
name = \"vhd\"
crate-type = [\"staticlib\", \"rlib\"]
";
    assert_eq!(
        lib_name(toml).as_deref(),
        Some("vhd"),
        "the package name is not the library name, and using it would look for \
         libam-img-vhd.a"
    );
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
