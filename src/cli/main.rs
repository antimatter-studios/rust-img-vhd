//! `rust-img-vhd`: the command-line tool for VHD images, one multi-call
//! binary.
//!
//! Installed as `rust-img-vhd` and linked as `img.vhd`; see `common` for
//! the dispatch and the output contract every tool shares, and `vhd` for
//! the tool itself.

// The shared plumbing is a library in waiting (see its module docs): its
// API is whole, and a piece this repository does not call yet is not dead,
// it is the part another format's tools will.
#[allow(dead_code)]
mod common;
mod vhd;

use std::process::ExitCode;

static FAMILY: common::Family = common::Family {
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
    common::main(&FAMILY)
}
