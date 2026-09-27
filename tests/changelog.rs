//! The changelog's shape and its agreement with `Cargo.toml`, checked rather
//! than remembered.
//!
//! rust-img-vhdx#63 is the defect this exists for, and it was found by a review bot rather
//! than by anything here: `Header` gained a required public field after
//! v0.3.5 and `pub fn encode_header(h: &Header)` makes that reachable — a
//! downstream has no way to build a `Header` except by struct literal, which
//! no longer compiled. The pending release was on course to be a patch.
//!
//! The issue notes the same shape was found independently in two sibling
//! repositories in the same month: `rust-partitions` (`Partition` gained two
//! fields at 0.4.1, the changelog recording one of them) and `rust-fs-ext4`
//! (`XattrEntry` gained a field and `Error` a variant, at 0.5.1, with
//! `[Unreleased]` empty). Three repositories, same mistake. The issue asks
//! for a release-checklist item; this is the same idea written as something
//! that runs, because a checklist nobody reads is a residue and the bot's
//! five accurate reports on the sibling did not enforce its own convention
//! either.
//!
//! WHAT A TEST CAN AND CANNOT SEE. It cannot tell whether a change to `src/`
//! broke an API — that needs the previous release's rustdoc to compare
//! against. It can insist the changelog **says so in a place the release
//! reads**: an entry marked `BREAKING` in a released section means the minor
//! moved. That turns "remember semver at release time" into a failure with a
//! line number, and leaves the judgement — is this breaking? — where it has
//! to be, with the person writing the entry.

use std::collections::BTreeMap;

fn changelog() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/CHANGELOG.md"))
        .expect("read CHANGELOG.md")
}

/// One `## [...]` section: its heading text and its body.
struct Section {
    name: String,
    body: String,
}

fn sections(text: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            out.push(Section {
                name: name.trim().to_owned(),
                body: String::new(),
            });
        } else if let Some(last) = out.last_mut() {
            last.body.push_str(line);
            last.body.push('\n');
        }
    }
    out
}

/// `[0.4.0] — 2026-09-27` → `(0, 4, 0)`. `None` for `[Unreleased]` and for
/// anything else this does not recognise, so an unparsed heading is skipped
/// rather than silently read as a version.
fn version_of(section: &str) -> Option<(u64, u64, u64)> {
    let inner = section.strip_prefix('[')?;
    let close = inner.find(']')?;
    let mut parts = inner[..close].split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn manifest_version() -> (u64, u64, u64) {
    let manifest: toml::Table =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .expect("read Cargo.toml")
            .parse()
            .expect("Cargo.toml parses as TOML");
    let text = manifest["package"]["version"]
        .as_str()
        .expect("[package].version is a string")
        .to_owned();
    version_of(&format!("[{text}]")).unwrap_or_else(|| panic!("unparsed version {text:?}"))
}

/// `[Unreleased]` grew two `### Added` blocks in a sibling before anyone
/// noticed, one per PR that added a section instead of adding to the one
/// already there.
#[test]
fn every_release_section_has_each_heading_at_most_once() {
    let text = changelog();
    let mut counts: Vec<(String, BTreeMap<String, usize>)> = Vec::new();
    for line in text.lines() {
        if let Some(section) = line.strip_prefix("## ") {
            counts.push((section.trim().to_owned(), BTreeMap::new()));
        } else if let Some(heading) = line.strip_prefix("### ") {
            if let Some((_, c)) = counts.last_mut() {
                *c.entry(heading.trim().to_owned()).or_default() += 1;
            }
        }
    }
    assert!(
        counts.iter().any(|(s, _)| s == "[Unreleased]"),
        "the scan found no [Unreleased] section, so it is not reading the file it guards"
    );
    let repeated: Vec<String> = counts
        .iter()
        .flat_map(|(section, c)| {
            c.iter()
                .filter(|(_, &n)| n > 1)
                .map(move |(h, n)| format!("{section}: ### {h} x{n}"))
        })
        .collect();
    assert!(
        repeated.is_empty(),
        "add entries under the heading that is already there: {repeated:?}"
    );
}

/// THE VERSION THIS CRATE PUBLISHES AND THE VERSION IT DOCUMENTS ARE ONE
/// NUMBER.
///
/// `release.yml` fires on a `v*.*.*` tag, so the tag, `Cargo.toml` and the
/// changelog's newest released section are three copies of the same fact kept
/// in three places. Two of them are in this repository and can be compared.
#[test]
fn the_manifest_version_is_the_newest_released_section() {
    let text = changelog();
    let newest = sections(&text)
        .into_iter()
        .find_map(|s| version_of(&s.name))
        .expect("the changelog has at least one released section");
    assert_eq!(
        manifest_version(),
        newest,
        "[package].version and the newest `## [x.y.z]` in CHANGELOG.md disagree. \
         A release is a tag, a manifest version and a changelog section saying \
         the same thing; two of the three are in this repository."
    );
}

/// Does this section body declare a break?
///
/// CASE-INSENSITIVE, AND THAT IS NOT A NICETY. The first version of this check
/// was `body.contains("BREAKING")`, matched against the uppercase spelling
/// this repository happens to use. The sibling `rust-img-vhd` writes it
/// lowercase -- `**\`Error::ReadOnly\` carries its cause** (breaking: match
/// \`ReadOnly(_)\`)` -- and an `Error` variant that gained a payload is as
/// breaking as anything here. So the guard would have passed that changelog
/// and let the release ship as a patch: a check that misses the very case it
/// was written for reports protection it is not providing, which is the defect
/// this whole file exists to catch, found inside the file itself.
///
/// The word, in any case, anywhere in the section. Deliberately loose: a false
/// positive costs a minor bump nobody needed, and a false negative costs a
/// consumer a build that stopped compiling on a patch.
fn marks_a_break(body: &str) -> bool {
    body.to_ascii_lowercase().contains("breaking")
}

/// A RELEASED SECTION THAT SAYS `BREAKING` BUMPED THE MINOR.
///
/// This is rust-img-vhdx#63 as an assertion. The crate header states the rule — "this is a
/// `0.x` crate, so the **minor** is the compatibility boundary: a minor bump
/// may break API, a patch never does" — and nothing enforced it.
///
/// The judgement stays with whoever writes the entry: the test does not decide
/// whether a change is breaking, it insists that an entry which already says so
/// is released as a minor. An entry that is breaking and does not say so is
/// beyond what any text scan can see, which is why `Error` is
/// `#[non_exhaustive]` — the most common source of that mistake here is not a
/// judgement call any more.
#[test]
fn a_released_section_that_breaks_api_bumped_the_minor() {
    let text = changelog();
    let released: Vec<(String, (u64, u64, u64), String)> = sections(&text)
        .into_iter()
        .filter_map(|s| version_of(&s.name).map(|v| (s.name, v, s.body)))
        .collect();

    // TWO CONTROLS, AND NEITHER NAMES A VERSION. A control that says "expect
    // 0.4.0" fails at the next release, and worse, it fails INSTEAD of the
    // rule -- so a mutation meant to test the rule proves nothing. These say
    // only that there is something to compare and something to compare it
    // against.
    assert!(
        released.len() >= 2,
        "fewer than two released sections, so there is no pair to compare; \
         found {:?}",
        released.iter().map(|(n, _, _)| n).collect::<Vec<_>>()
    );
    // NO CONTROL DEMANDING A REAL `BREAKING` ENTRY, and the first version of
    // this file had one. It asserted the changelog contained a marker, so that
    // a scan matching nothing could not pass vacuously -- reasonable, and
    // wrong: a crate whose released history has broken nothing would have to
    // invent a break to satisfy it. This crate is exactly that (only additions
    // since v0.3.5), and porting the file failed on its CONTROL rather than on
    // the rule -- which is the same "a check that cannot fail" defect the
    // control was written to prevent, arrived at from the other side.
    //
    // `a_break_is_recognised_however_it_is_spelled` is the honest form: it
    // proves the scan works against bodies it is handed, rather than requiring
    // the repository to supply one.

    // Newest first, so each section's predecessor is the next one down.
    for pair in released.windows(2) {
        let (name, (major, minor, _), body) = &pair[0];
        let (previous_name, (previous_major, previous_minor, _), _) = &pair[1];
        if !marks_a_break(body) {
            continue;
        }
        assert!(
            (major, minor) > (previous_major, previous_minor),
            "`## {name}` carries a BREAKING entry but is a patch on \
             `## {previous_name}`. This crate's own header says the minor is \
             the compatibility boundary for a 0.x crate, so a section that \
             breaks API is x.(y+1).0 -- see rust-img-vhdx#63, where a required public field \
             was added to `Header` and the pending release was going to be \
             0.3.6."
        );
    }
}

/// EVERY RELEASED SECTION HAS A COMPARE LINK, because that is a third copy of
/// the same fact and it was the one that had gone stale.
///
/// Found by this file on its first run: `[Unreleased]` still compared
/// `v0.3.4...HEAD` with 0.3.5 released, and there was no `[0.3.5]` definition
/// at all — so the heading rendered as literal brackets. Nobody reads the
/// bottom of a changelog, which is exactly why it needs a test rather than
/// attention.
#[test]
fn every_released_section_has_a_compare_link() {
    let text = changelog();
    let defined: Vec<String> = text
        .lines()
        .filter_map(|l| l.strip_prefix('['))
        .filter_map(|l| l.split_once("]: http"))
        .map(|(name, _)| name.to_owned())
        .collect();
    assert!(
        defined.iter().any(|d| d == "Unreleased"),
        "no link definitions were found at all, so this is not reading the file it guards"
    );
    let missing: Vec<String> = sections(&text)
        .into_iter()
        .filter(|s| version_of(&s.name).is_some())
        .map(|s| {
            s.name
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .filter(|v| !defined.contains(v))
        .collect();
    assert!(
        missing.is_empty(),
        "these released sections have no `[x.y.z]: <url>` definition, so their \
         headings render as literal brackets: {missing:?}"
    );
}

/// The parser's own edges, so a heading it cannot read is skipped rather than
/// mistaken for a version — which would let a real release escape the checks
/// above while they still passed.
#[test]
fn a_section_heading_is_read_as_a_version_or_not_at_all() {
    assert_eq!(version_of("[0.4.0] — 2026-09-27"), Some((0, 4, 0)));
    assert_eq!(version_of("[10.2.30]"), Some((10, 2, 30)));
    for unreadable in [
        "[Unreleased]",
        "[0.4]",
        "[0.4.0.1]",
        "[v0.4.0]",
        "[0.4.0-rc1]",
        "0.4.0",
        "[]",
        "[a.b.c]",
    ] {
        assert_eq!(version_of(unreadable), None, "{unreadable:?}");
    }
}

/// The marker scan's own spellings, because the family uses more than one and
/// the guard is worthless against the ones it cannot see.
#[test]
fn a_break_is_recognised_however_it_is_spelled() {
    for body in [
        "- **Thing changed** *(#1 — BREAKING: match `X(_)`.)*",
        "- **`Error::ReadOnly` carries its cause** (breaking: match `ReadOnly(_)`).",
        "- Something *(Breaking change.)*",
        "- a bREaKiNg change",
    ] {
        assert!(marks_a_break(body), "{body:?}");
    }
    for body in [
        "- **Thing changed**, no compatibility note at all.",
        "- The brake was fixed.",
        "",
    ] {
        assert!(!marks_a_break(body), "{body:?}");
    }
}
