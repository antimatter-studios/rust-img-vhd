//! CLI inspector for VHD images.
//!
//! Usage:
//!   vhd_tool info <file>
//!   vhd_tool read <file> <offset> <len>     hex dump
//!   vhd_tool create-fixed <file> <size>     fresh fixed VHD
//!   vhd_tool create-dynamic <file> <size> [--block-size N]
//!                                           fresh dynamic VHD (2 MiB blocks)
//!   vhd_tool write <file> <offset> <input>  write <input>'s bytes at <offset>

use std::process::ExitCode;
use vhd::VhdReader;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let rc = match args.get(1).map(String::as_str) {
        Some("info") => cmd_info(&args[2..]),
        Some("read") => cmd_read(&args[2..]),
        Some("create-fixed") => cmd_create_fixed(&args[2..]),
        Some("create-dynamic") => cmd_create_dynamic(&args[2..]),
        Some("write") => cmd_write(&args[2..]),
        _ => {
            eprintln!(
                "vhd_tool — VHD inspector\n\n\
                 Usage:\n\
                 \tvhd_tool info <file>\n\
                 \tvhd_tool read <file> <offset> <len>\n\
                 \tvhd_tool create-fixed <file> <size>\n\
                 \tvhd_tool create-dynamic <file> <size> [--block-size N]\n\
                 \tvhd_tool write <file> <offset> <input>\n"
            );
            Ok(())
        }
    };
    match rc {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("vhd_tool: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_info(args: &[String]) -> Result<(), String> {
    if args.len() != 1 {
        return Err("info: expected <file>".into());
    }
    let r = VhdReader::open(&args[0]).map_err(|e| e.to_string())?;
    let f = r.footer();
    println!("disk_type        : {:?}", r.disk_type());
    println!(
        "virtual_size     : {} bytes ({:.2} MiB)",
        r.virtual_size(),
        r.virtual_size() as f64 / (1024.0 * 1024.0)
    );
    println!("block_size       : {} bytes", r.block_size());
    println!("file_format_ver  : {:#x}", f.file_format_version);
    println!("has_parent       : {}", r.has_parent());
    Ok(())
}

fn cmd_read(args: &[String]) -> Result<(), String> {
    if args.len() != 3 {
        return Err("read: expected <file> <offset> <len>".into());
    }
    let r = VhdReader::open(&args[0]).map_err(|e| e.to_string())?;
    let offset = parse_u64(&args[1])?;
    let len = parse_u64(&args[2])?;
    if len > 4 * 1024 * 1024 {
        return Err("len too large (cap 4 MiB)".into());
    }
    let mut buf = vec![0u8; len as usize];
    r.read_at(offset, &mut buf).map_err(|e| e.to_string())?;
    hex_dump(offset, &buf);
    Ok(())
}

fn cmd_create_fixed(args: &[String]) -> Result<(), String> {
    if args.len() != 2 {
        return Err("create-fixed: expected <file> <size>".into());
    }
    let size = parse_u64(&args[1])?;
    let r = VhdReader::create_fixed(&args[0], size).map_err(|e| e.to_string())?;
    println!(
        "created fixed VHD: {} ({} bytes virtual)",
        args[0],
        r.virtual_size()
    );
    Ok(())
}

/// The default dynamic block size, the one Hyper-V and qemu-img use.
const DEFAULT_BLOCK_SIZE: u64 = 2 * 1024 * 1024;

fn cmd_create_dynamic(args: &[String]) -> Result<(), String> {
    let (file, size, block_size) = match args {
        [file, size] => (file, size, DEFAULT_BLOCK_SIZE),
        [file, size, flag, n] if flag == "--block-size" => (file, size, parse_u64(n)?),
        _ => return Err("create-dynamic: expected <file> <size> [--block-size N]".into()),
    };
    let size = parse_u64(size)?;
    let block_size =
        u32::try_from(block_size).map_err(|_| format!("block size {block_size} is too large"))?;
    let r = VhdReader::create_dynamic(file, size, block_size).map_err(|e| e.to_string())?;
    println!(
        "created dynamic VHD: {file} ({} bytes virtual, {} byte blocks)",
        r.virtual_size(),
        r.block_size()
    );
    Ok(())
}

fn cmd_write(args: &[String]) -> Result<(), String> {
    if args.len() != 3 {
        return Err("write: expected <file> <offset> <input>".into());
    }
    let offset = parse_u64(&args[1])?;
    let data = std::fs::read(&args[2]).map_err(|e| format!("reading {}: {e}", args[2]))?;
    let r = VhdReader::open_rw(&args[0]).map_err(|e| e.to_string())?;
    r.write_at(offset, &data).map_err(|e| e.to_string())?;
    r.flush_writes().map_err(|e| e.to_string())?;
    println!("wrote {} bytes at {offset}", data.len());
    Ok(())
}

fn parse_u64(s: &str) -> Result<u64, String> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| format!("invalid hex {s:?}: {e}"))
    } else {
        s.parse::<u64>()
            .map_err(|e| format!("invalid decimal {s:?}: {e}"))
    }
}

fn hex_dump(start: u64, bytes: &[u8]) {
    for (i, line) in bytes.chunks(16).enumerate() {
        let off = start + (i as u64) * 16;
        print!("{off:08x}  ");
        for (j, b) in line.iter().enumerate() {
            print!("{b:02x}");
            if j == 7 {
                print!(" ");
            }
            print!(" ");
        }
        for _ in line.len()..16 {
            print!("   ");
        }
        print!(" |");
        for b in line {
            let c = *b;
            print!(
                "{}",
                if (0x20..0x7f).contains(&c) {
                    c as char
                } else {
                    '.'
                }
            );
        }
        println!("|");
    }
}
