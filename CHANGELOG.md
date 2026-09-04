# Changelog

Notable changes to `am-img-vhd`, newest first. This is a `0.x` crate, so the
**minor** is the compatibility boundary: a minor bump may break API, a patch
never does.


## [Unreleased]

## [0.3.4] — 2026-09-04

### Changed

- **The VHD layout is written down once.** Footer and dynamic-header field
  offsets had been open-coded at each use, so a reader had to re-derive the
  layout at every site and a drifted copy would have gone unnoticed. There is
  now a single `format` module holding the offsets, the CHS geometry rules and
  the ones-complement checksum, with compile-time assertions tying the
  addressing ceilings together.

  **The fixtures were deliberately NOT rewritten to use it.** A test that
  builds its image from the same constants the parser reads cannot catch a
  wrong constant. Keeping the fixtures independent costs some duplication and
  buys a second opinion — measured by mutation, not assumed.

- **The sector-bitmap arithmetic has names**, and the question of which bound
  it is checked against is settled rather than restated differently at each
  site.

## [0.3.3] — 2026-08-29

### Fixed

- **The geometry is checked against something other than itself**, and the
  sparse allocator is locked, so two writers extending the same image cannot
  hand out the same block.

### Added

- `chore` tasks own this crate's build, and the code-review report is recorded
  in the repo.
- The github-guard hook set replaces the hand-rolled pre-commit hooks.

## [0.3.2] — 2026-06-21

### Changed

- The publish job clones its path-dependency siblings, pinned to a tag rather
  than tracking a branch, and publishing is gated on the disk-image validator
  cross-check. A release built from a floating dependency is not reproducible.

## [0.3.1] — 2026-06-09

### Changed

- Pinned toolchain moves from 1.94.1 to 1.95.0, in lockstep with the rest of
  the family. A straggler links two copies of `_rust_eh_personality` into any
  consumer that binds both.

## [0.3.0] — 2026-06-01

### Added

- Cross-validation against an external disk-image validator, so the images this
  crate writes are checked by something that did not write them.
- Unit tests for footer, dynamic header and geometry; fixed-VHD corruption and
  round-trip tests.

## [0.2.0] — 2026-05-12

### Added

- Device-backed reader and the dynamic-VHD write path.

### Added

- Release-on-tag pipeline using trusted publishing, and CI (test, fmt, clippy).

### Changed

- `am-fs-core` dependency moves to 0.2.

[Unreleased]: https://github.com/antimatter-studios/rust-img-vhd/compare/v0.3.4...HEAD
[0.3.4]: https://github.com/antimatter-studios/rust-img-vhd/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/antimatter-studios/rust-img-vhd/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/antimatter-studios/rust-img-vhd/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/antimatter-studios/rust-img-vhd/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/antimatter-studios/rust-img-vhd/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/antimatter-studios/rust-img-vhd/releases/tag/v0.2.0
