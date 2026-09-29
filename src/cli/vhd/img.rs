//! `img.vhd <image> <verb>`: an errand inside a VHD disk image, without a
//! hypervisor.
//!
//! The verbs are the shared set for disk images: `info`/`get`, `read`,
//! `write`, `create`, `resize`, `set`. Metadata is JSON (or `--text`); the
//! guest's bytes are raw. `read` with no range streams the whole virtual
//! disk, so converting to a raw image is reading it. A verb the library
//! cannot do yet still exists and answers `not implemented` with exit
//! status 3, so a script moved between formats fails loudly instead of
//! meaning something else.

use std::ffi::OsString;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use clap::{value_parser, Arg, ArgMatches, Command as Cmd};

use crate::common::{CliError, Json, Outcome, Tool};
use vhd::format::footer_offsets as at;
use vhd::{DiskType, VhdReader};

pub const TOOL: Tool = Tool {
    name: "img.vhd",
    verb: "img",
    section: 1,
    usage_exit: crate::common::output::EXIT_USAGE,
    about: "Report, read and write a VHD disk image without a hypervisor",
    command,
    run,
};

/// The canonical keys every `img.<fmt>` answers, in the shared order.
/// The format's own fields are nested under `vhd`.
pub const KEYS: &[&str] = &[
    "format",
    "virtual_size",
    "block_size",
    "backing",
    "dirty",
    "vhd",
];

/// How much of the guest is moved at a time.
const CHUNK: usize = 1 << 20;

fn command() -> Cmd {
    Cmd::new("img.vhd")
        .about("Report, read and write a VHD disk image without a hypervisor")
        .long_about(
            "Work inside a VHD (Virtual PC / Hyper-V) disk image directly: fixed, dynamic \
             and differencing images, no hypervisor and no conversion tool.\n\n\
             Metadata is JSON on stdout (--text for people); `read` writes the guest's raw \
             bytes, the whole virtual disk when no range is given, so converting to a raw \
             image is reading it. A failure is {\"error\": \"...\", \"code\": N} on stderr, \
             N being the exit status: 1 failed, 2 wrong command line, 3 not implemented.",
        )
        .arg(
            Arg::new("image")
                .value_name("IMAGE")
                .help("The VHD image file")
                .value_parser(value_parser!(OsString))
                .required(true),
        )
        .args(crate::common::format_args().map(|a| a.global(true)))
        .subcommand_required(true)
        .subcommand(key_command(
            "info",
            "Report the image's properties, or one of them",
        ))
        .subcommand(key_command(
            "get",
            "The same as info: every property, or one of them",
        ))
        .subcommand(
            Cmd::new("read")
                .about("Write the guest's bytes to stdout, or to a file with -o: the whole disk, or a range")
                .arg(byte_count("offset", "Where to start, in the guest (default 0)"))
                .arg(byte_count(
                    "length",
                    "How many bytes (default: to the end of the virtual disk)",
                ))
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .value_name("FILE")
                        .value_parser(value_parser!(OsString))
                        .help("Write here instead of stdout (zero runs are left sparse)"),
                )
                .after_help(
                    "Examples:\n  img.vhd disk.vhd read -o disk.raw\n  \
                     img.vhd disk.vhd read --offset 0 --length 512 | xxd\n  \
                     img.vhd disk.vhd read --offset 1M --length 64K > chunk.bin",
                ),
        )
        .subcommand(
            Cmd::new("write")
                .about("Write the bytes on stdin into the guest at an offset (not implemented yet)")
                .arg(byte_count("offset", "Where to write, in the guest").required(true))
                .after_help(
                    "Examples:\n  img.vhd disk.vhd write --offset 0 < mbr.bin\n\n\
                     Answers `not implemented` (exit 3) until this tool's write verb lands.",
                ),
        )
        .subcommand(
            Cmd::new("create")
                .about("Create a new, empty image (not implemented yet)")
                .arg(Arg::new("size").value_name("SIZE").required(true))
                .after_help(
                    "Examples:\n  img.vhd new.vhd create 64M\n\n\
                     Answers `not implemented` (exit 3) until this tool's create verb lands.",
                ),
        )
        .subcommand(
            Cmd::new("set")
                .about("Change a property (not implemented)")
                .arg(Arg::new("key").value_name("KEY").required(true))
                .arg(Arg::new("value").value_name("VALUE").required(true))
                .after_help(
                    "Examples:\n  img.vhd disk.vhd set vhd.saved_state false\n\n\
                     Answers `not implemented` (exit 3): the library changes no footer field.",
                ),
        )
        .subcommand(
            Cmd::new("resize")
                .about("Grow or shrink the virtual disk (not implemented)")
                .arg(Arg::new("size").value_name("SIZE").required(true))
                .after_help(
                    "Examples:\n  img.vhd disk.vhd resize 20G\n\n\
                     Answers `not implemented` (exit 3): the library has no resize.",
                ),
        )
        .after_help(
            "Examples:\n  img.vhd disk.vhd info\n  \
             img.vhd disk.vhd get virtual_size --text\n  \
             img.vhd disk.vhd read -o disk.raw\n  \
             img.vhd disk.vhd read --offset 0 --length 512 | xxd",
        )
}

fn byte_count(id: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .long(id)
        .value_name("BYTES")
        .help(help)
        .value_parser(super::size::parse)
}

fn key_command(name: &'static str, about: &'static str) -> Cmd {
    Cmd::new(name)
        .about(about)
        .arg(
            Arg::new("key")
                .value_name("KEY")
                .help(format!("One of: {} (or vhd.<field>)", KEYS.join(", "))),
        )
        .after_help(format!(
            "Examples:\n  img.vhd disk.vhd {name}\n  \
             img.vhd disk.vhd {name} virtual_size --text\n  \
             img.vhd disk.vhd {name} vhd.disk_type"
        ))
}

fn run(matches: &ArgMatches) -> Result<Outcome, CliError> {
    let image = Path::new(
        matches
            .get_one::<OsString>("image")
            .expect("clap requires the image"),
    );
    let (verb, sub) = matches.subcommand().expect("clap requires a verb");
    match verb {
        "info" | "get" => get(image, sub.get_one::<String>("key").map(String::as_str)),
        "read" => read(
            image,
            sub.get_one::<u64>("offset").copied(),
            sub.get_one::<u64>("length").copied(),
            sub.get_one::<OsString>("output").map(PathBuf::from),
        ),
        "write" => Err(CliError::not_implemented(
            "write: this tool's write verb has not landed yet",
        )),
        "create" => Err(CliError::not_implemented(
            "create: this tool's create verb has not landed yet",
        )),
        "set" => Err(CliError::not_implemented(
            "set: this library changes no footer or header field",
        )),
        "resize" => Err(CliError::not_implemented(
            "resize: this library cannot resize a VHD",
        )),
        other => unreachable!("clap knows no verb {other}"),
    }
}

fn vhd_error(image: &Path, e: vhd::Error) -> CliError {
    CliError::failed(format!("{}: {e}", image.display()))
}

fn open(image: &Path) -> Result<VhdReader, CliError> {
    VhdReader::open(image).map_err(|e| vhd_error(image, e))
}

fn disk_type_name(t: DiskType) -> &'static str {
    match t {
        DiskType::Fixed => "fixed",
        DiskType::Dynamic => "dynamic",
        DiskType::Differencing => "differencing",
    }
}

fn be_u16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

fn be_u32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// A four-character code (`qemu`, `vpc `, `Wi2k`) as text, its padding
/// dropped. Bytes that are not printable ASCII are shown as hex instead,
/// so the value is always valid JSON text and never silently lossy.
fn four_cc(bytes: &[u8]) -> String {
    if bytes
        .iter()
        .all(|b| b.is_ascii_graphic() || *b == b' ' || *b == 0)
    {
        String::from_utf8_lossy(bytes)
            .trim_end_matches([' ', '\0'])
            .to_string()
    } else {
        format!("0x{}", hex(bytes))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The standard 8-4-4-4-12 form of a 16-byte id, in the byte order it is
/// stored.
fn uuid_text(id: &[u8; 16]) -> String {
    let h = hex(id);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Seconds between the Unix epoch and the VHD epoch, 2000-01-01T00:00:00Z.
const VHD_EPOCH_UNIX: u64 = 946_684_800;

/// The envelope: the shared keys first, the format's own under `vhd`.
pub fn envelope(r: &VhdReader) -> Json {
    let f = r.footer();
    let b = r.footer_bytes();
    let dynamic = r.dynamic_header();
    let parent = dynamic.filter(|_| r.disk_type() == DiskType::Differencing);
    let creator_version = be_u32(b, at::CREATOR_VERSION);
    let version = f.file_format_version;
    Json::object([
        ("format", Json::from("vhd")),
        ("virtual_size", Json::from(r.virtual_size())),
        (
            "block_size",
            // A fixed image has no blocks: null, not a number a script
            // would do arithmetic with.
            dynamic.map_or(Json::Null, |d| Json::from(d.block_size)),
        ),
        ("backing", Json::from(parent.map(|h| h.parent_name.clone()))),
        // The format records no dirty state: a VHD has no journal, no
        // refcounts and no in-use flag.
        ("dirty", Json::from(false)),
        (
            "vhd",
            Json::object([
                ("disk_type", Json::from(disk_type_name(r.disk_type()))),
                ("original_size", Json::from(f.original_size)),
                (
                    "geometry",
                    Json::object([
                        ("cylinders", Json::from(be_u16(b, at::DISK_GEOMETRY))),
                        ("heads", Json::from(u16::from(b[at::DISK_GEOMETRY + 2]))),
                        (
                            "sectors_per_track",
                            Json::from(u16::from(b[at::DISK_GEOMETRY + 3])),
                        ),
                    ]),
                ),
                (
                    "creator",
                    Json::object([
                        (
                            "application",
                            Json::from(four_cc(
                                &b[at::CREATOR_APPLICATION..at::CREATOR_APPLICATION + 4],
                            )),
                        ),
                        (
                            "version",
                            Json::from(format!(
                                "{}.{}",
                                creator_version >> 16,
                                creator_version & 0xffff
                            )),
                        ),
                        (
                            "host_os",
                            Json::from(four_cc(&b[at::CREATOR_HOST_OS..at::CREATOR_HOST_OS + 4])),
                        ),
                    ]),
                ),
                (
                    "file_format_version",
                    Json::from(format!("{}.{}", version >> 16, version & 0xffff)),
                ),
                ("unique_id", Json::from(uuid_text(&f.unique_id))),
                (
                    "timestamp",
                    Json::from(u64::from(be_u32(b, at::TIMESTAMP)) + VHD_EPOCH_UNIX),
                ),
                ("saved_state", Json::from(b[at::SAVED_STATE] != 0)),
                (
                    "footer_recovered_from_mirror",
                    Json::from(r.footer_recovered_from_mirror()),
                ),
                (
                    "block_count",
                    dynamic.map_or(Json::Null, |d| Json::from(d.max_table_entries)),
                ),
                (
                    "parent",
                    parent.map_or(Json::Null, |h| {
                        Json::object([
                            ("name", Json::from(h.parent_name.as_str())),
                            ("unique_id", Json::from(uuid_text(&h.parent_unique_id))),
                        ])
                    }),
                ),
            ]),
        ),
    ])
}

fn get(image: &Path, key: Option<&str>) -> Result<Outcome, CliError> {
    let all = envelope(&open(image)?);
    let Some(key) = key else {
        return Ok(Outcome::report(all));
    };
    let mut value = Some(&all);
    for part in key.split('.') {
        value = value.and_then(|v| v.get(part));
    }
    let Some(value) = value else {
        return Err(CliError::usage(format!(
            "no key {key:?}; the keys are {} (and vhd.<field>)",
            KEYS.join(", ")
        )));
    };
    let text = value.to_text();
    Ok(Outcome::report(Json::object([(key, value.clone())])).with_text(text))
}

/// The range a `read` covers: `offset` (default 0) for `length` bytes
/// (default: to the end), refused whole if any of it is past the end, so
/// nothing is written for a range that cannot be served.
fn range(size: u64, offset: Option<u64>, length: Option<u64>) -> Result<(u64, u64), CliError> {
    let offset = offset.unwrap_or(0);
    if offset > size {
        return Err(CliError::failed(format!(
            "--offset {offset} is past the end of the {size}-byte virtual disk"
        )));
    }
    let length = length.unwrap_or(size - offset);
    if offset.checked_add(length).is_none_or(|end| end > size) {
        return Err(CliError::failed(format!(
            "{length} bytes at {offset} run past the end of the {size}-byte virtual disk"
        )));
    }
    Ok((offset, length))
}

/// Stream the guest's bytes. Each chunk is read before it is written, so
/// an image that turns out unreadable part-way stops with status 1; what
/// can be refused up front (no image, a range past the end) is refused
/// before a byte is written. `-o FILE` writes `FILE.partial` and renames
/// it, so FILE is never left half written, and skips runs of zeros so a
/// mostly empty disk makes a sparse file.
fn read(
    image: &Path,
    offset: Option<u64>,
    length: Option<u64>,
    output: Option<PathBuf>,
) -> Result<Outcome, CliError> {
    let r = open(image)?;
    let (offset, length) = range(r.virtual_size(), offset, length)?;
    let mut buf = vec![0u8; CHUNK];
    match output {
        None => {
            let mut out = std::io::stdout().lock();
            let mut at = offset;
            let end = offset + length;
            while at < end {
                let n = CHUNK.min((end - at) as usize);
                r.read_at(at, &mut buf[..n])
                    .map_err(|e| vhd_error(image, e))?;
                if let Err(e) = out.write_all(&buf[..n]) {
                    // A closed pipe (`| head -c 512`) is the reader's
                    // choice, not a failure.
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        return Ok(Outcome::done());
                    }
                    return Err(CliError::failed(format!("write stdout: {e}")));
                }
                at += n as u64;
            }
            if let Err(e) = out.flush() {
                if e.kind() != std::io::ErrorKind::BrokenPipe {
                    return Err(CliError::failed(format!("write stdout: {e}")));
                }
            }
        }
        Some(dest) => {
            let mut partial = dest.as_os_str().to_owned();
            partial.push(".partial");
            let partial = PathBuf::from(partial);
            let copied = (|| -> Result<(), CliError> {
                let io = |e: std::io::Error| {
                    CliError::failed(format!("write {}: {e}", partial.display()))
                };
                let mut f = std::fs::File::create(&partial).map_err(io)?;
                let mut at = offset;
                let end = offset + length;
                while at < end {
                    let n = CHUNK.min((end - at) as usize);
                    r.read_at(at, &mut buf[..n])
                        .map_err(|e| vhd_error(image, e))?;
                    if buf[..n].iter().all(|b| *b == 0) {
                        f.seek(SeekFrom::Current(n as i64)).map_err(io)?;
                    } else {
                        f.write_all(&buf[..n]).map_err(io)?;
                    }
                    at += n as u64;
                }
                // A trailing run of zeros was skipped, not written: the
                // length is set, not implied.
                f.set_len(length).map_err(io)?;
                f.sync_all().map_err(io)
            })();
            if let Err(e) = copied {
                let _ = std::fs::remove_file(&partial);
                return Err(e);
            }
            std::fs::rename(&partial, &dest)
                .map_err(|e| CliError::failed(format!("rename to {}: {e}", dest.display())))?;
        }
    }
    Ok(Outcome::done())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_defaults_to_the_whole_disk_and_refuses_the_far_side() {
        assert_eq!(range(1000, None, None).ok(), Some((0, 1000)));
        assert_eq!(range(1000, Some(10), None).ok(), Some((10, 990)));
        assert_eq!(range(1000, None, Some(10)).ok(), Some((0, 10)));
        assert_eq!(range(1000, Some(1000), None).ok(), Some((1000, 0)));
        assert!(range(1000, Some(1001), None).is_err());
        assert!(range(1000, Some(990), Some(11)).is_err());
        assert!(range(1000, Some(u64::MAX), Some(2)).is_err());
    }

    #[test]
    fn identifiers_read_the_way_the_format_writes_them() {
        assert_eq!(four_cc(b"vpc "), "vpc");
        assert_eq!(four_cc(b"Wi2k"), "Wi2k");
        assert_eq!(four_cc(b"\x01\x02ab"), "0x01026162");
        assert_eq!(
            uuid_text(&[
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff
            ]),
            "00112233-4455-6677-8899-aabbccddeeff"
        );
    }
}
