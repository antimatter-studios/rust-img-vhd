# vhd

Pure-Rust reader for the Microsoft VHD (Virtual Hard Disk) format. Spec
implemented from Microsoft's published whitepaper; no GPL code is
copied or linked. Exposes a Rust API and a C ABI suitable for FFI from
C/C++/Go/Swift.

## Status

- [x] Fixed VHD (footer at end of file, plain pass-through to data)
- [x] Dynamic VHD (footer + dynamic header + BAT + sparse blocks)
- [x] Differencing VHD (parent VHD chain, fall-through reads)
- [x] `BlockRead` + `BlockDevice` impl via `am-fs-core`
- [x] Reader is generic over `Arc<dyn BlockDevice>`; path-based `open` /
  `open_rw` / `create_fixed` wrap a `FileDevice` internally, and the
  `open_on_device` / `open_rw_on_device` constructors stack the VHD layer
  on top of any caller-supplied device (e.g. an FSKit block resource).
- [x] C ABI: `vhd_open` / `vhd_open_rw` / `vhd_create_fixed` /
  `vhd_open_on_device` / `vhd_open_rw_on_device` — all returning
  `*mut FsCoreDevice`.
- [x] Write support — fixed VHDs (`open_rw`, `create_fixed`, pass-through
  `write_at`)
- [x] Dynamic write path (BAT mutation, tail-allocated blocks, footer
  mirror rewrite). Crash-safety order on allocation:
  data → bitmap → BAT entry → footer mirror, with `dev.flush()` between
  each step.
  - [ ] Differencing write path — still TODO. `write_at` on a
    differencing reader returns `Error::ReadOnly`.

## Layout

```
src/
  lib.rs       public API
  error.rs     Error / Result
  footer.rs       512-byte footer at end of file (cookie "conectix")
  footer_build.rs footer composition for newly-created fixed VHDs
  dynamic.rs      1024-byte dynamic header (cookie "cxsparse") + BAT walker
  reader.rs       VhdReader — open, open_rw, create_fixed, BlockRead/BlockDevice impls
  capi.rs         C ABI returning FsCoreDevice handles
  bin/
    vhd_tool.rs   CLI: info, read, create-fixed
tests/
  synthetic.rs   hand-built fixtures: fixed, dynamic, differencing
include/
  vhd.h          C ABI header
```

## Spec

Microsoft's *Virtual Hard Disk Image Format Specification* (October
2006). The format is small enough that a single annotated source file
matches the spec sections one-to-one; see `footer.rs` and `dynamic.rs`.

## Verifying a release

From the next release onward, every version published to crates.io is
also attached to the GitHub release for its tag, with a build-provenance
attestation signed by this repository's release workflow. It proves the
crate was built by `.github/workflows/release.yml` from a commit in this
repository, not uploaded from someone's machine. To check the crates.io
download of version `X.Y.Z`:

```sh
curl -sSfLo am-img-vhd-X.Y.Z.crate https://static.crates.io/crates/am-img-vhd/am-img-vhd-X.Y.Z.crate
gh attestation verify am-img-vhd-X.Y.Z.crate \
  --repo antimatter-studios/rust-img-vhd \
  --signer-workflow antimatter-studios/rust-img-vhd/.github/workflows/release.yml
```

The workflow refuses to attest a `.crate` whose sha256 differs from the
checksum crates.io records for that version, so the file on the release
page and the crates.io download are the same bytes.

## License

MIT.
