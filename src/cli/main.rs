//! `rust-img-vhd`: the command-line tool for VHD images, one multi-call
//! binary.
//!
//! Installed as `rust-img-vhd` and linked as `img.vhd`. The dispatch and the
//! output contract every tool shares are `fs_core::cli` (am-fs-core's `cli`
//! feature); `vhd` is the tool itself.

mod vhd;

use fs_core::cli;
use std::process::ExitCode;

static FAMILY: cli::Family = cli::Family {
    repo: "rust-img-vhd",
    crate_name: env!("CARGO_PKG_NAME"),
    version: env!("CARGO_PKG_VERSION"),
    about: "VHD tools: report, read and write a VHD disk image without a hypervisor",
    install_hints: &[
        "`chore cli:install` from a checkout of this repository",
        "`brew install antimatter-studios/tap/rust-img-vhd`",
    ],
    tools: &[vhd::img::TOOL],
};

fn main() -> ExitCode {
    cli::main(&FAMILY)
}
