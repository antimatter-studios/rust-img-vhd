# Features

What this crate does today, what it refuses, and what is coming. **Every
pull request that adds, fixes, refuses or removes behaviour updates its row
here, in the same pull request** (AGENTS.md). The reasoning behind each change
is in [CHANGELOG.md](../CHANGELOG.md).

**Since** is the release a row's current state shipped in, with the issue or
pull request the changelog cites for it. Work merged after the last release
is **Unreleased (#N)** until the next one. **Tracking** names the issue for
anything not finished.

States:

- **Supported**: works, and is checked against `qemu-img` (VHD is its `vpc`
  format).
- **Experimental**: works in every test, but is new.
- **Partial**: works for part of the case, and the row says which part.
- **Refused**: recognised and refused by name, rather than misread.
- **Not supported**: neither read nor refused by name.
- **Upcoming**: an open issue with a plan.

## Reading

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Fixed images: the footer at the end, data passed through | Supported | 0.2.0 | | `synthetic.rs`, `qemu_validation.rs` |
| Dynamic images: dynamic header, BAT, sparse blocks | Supported | 0.2.0 | | `synthetic.rs`, `qemu_validation.rs` |
| Blocks the reference tool packed end to end | Supported | 0.4.0 | | `qemu_validation.rs` |
| Differencing images: the parent chain, reads falling through | Supported | 0.2.0 | | `synthetic.rs`, `tests/cli/test-differencing.sh` |
| A differencing image opened on a raw device, with no path to resolve its parent from | Refused | 0.2.0 | | |
| A damaged trailing footer, recovered from the mirror as the reference tool does | Supported | 0.4.0 (#93) | | `qemu_validation.rs` |
| A footer with a bad cookie, a bad checksum or an unknown disk type | Refused | 0.3.0 | | `corruption.rs` |
| A BAT entry pointing into the metadata | Refused | 0.4.0 | | `qemu_validation.rs` |
| CHS geometry, checked against the reference tool's | Supported | 0.3.3 | | `reference_geometry.rs`, `qemu_validation.rs` |
| Fuzzed footer, dynamic-header and BAT parsers | Supported | 0.4.0 (#93) | | `fuzz_decoders.rs` |

## Writing

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Writes to fixed images | Supported | 0.2.0 | | `corruption.rs`, `qemu_validation.rs` |
| Writes to dynamic images: BAT allocation, bitmaps, the footer mirror, flushed in a crash-safe order | Supported | 0.2.0; checked by `qemu-img` 0.4.0 (#46) | | `qemu_validation.rs` |
| Creating a fixed image, its size rounded up to a CHS geometry | Supported | 0.2.0; rounding 0.4.0 | | `qemu_validation.rs` |
| Creating a dynamic image (`create_dynamic`) | Supported | 0.4.0 (#46) | | `qemu_validation.rs` |
| Writes to differencing images | Refused (`Error::ReadOnly`) | 0.2.0 | | `tests/cli/test-differencing.sh` |
| Resizing an image | Not supported | | | `tests/cli/test-unsupported.sh` |

## Interfaces

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Rust API (`VhdReader`), over a path or any `rust-fs-core` device | Supported | 0.2.0 | | `synthetic.rs` |
| C ABI returning `FsCoreDevice` handles | Supported | 0.2.0 | | `header_names_the_built_library.rs` |
| `img.vhd` `info`/`get`, `read`, `write`, `create` (`--features cli`) | Supported | 0.5.0 | | `cli_write.rs`, `tests/cli/test-info.sh`, `tests/cli/test-read.sh`, `tests/cli/test-write.sh`, `tests/cli/test-create.sh` |
| `img.vhd` `resize`, `set`, and `write` on a differencing image | Not supported (`not implemented`, exit 3) | 0.5.0 | | `tests/cli/test-unsupported.sh`, `tests/cli/test-differencing.sh` |
| `rust-img-vhd doctor`, man pages, shell completions | Supported | 0.5.0 | | `tests/cli/test-names.sh`, `tests/cli/test-docs.sh` |
