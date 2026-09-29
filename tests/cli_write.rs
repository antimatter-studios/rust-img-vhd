//! `img.vhd write` refuses input that cannot be written before writing any
//! of it, and leaves the image as it was.
//!
//! These were `vhd_tool write`'s tests, rewritten against the tool that
//! replaced it. `vhd_tool` used to read the whole input into memory and
//! then let `write_at` report it out of bounds, so a large input -- or an
//! image selected as its own input -- cost its full size in memory to end
//! in an error. `img.vhd` reads its input from stdin: a regular file
//! redirected there is measured before a byte is read, and a pipe is read
//! no further than one byte past what fits.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const TOOL: &str = env!("CARGO_BIN_EXE_rust-img-vhd");

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("img-vhd-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn img(args: &[&str]) -> Command {
    let mut cmd = Command::new(TOOL);
    cmd.arg("img").args(args);
    cmd
}

fn create_dynamic(vhd: &Path, size: &str) {
    let made = img(&[vhd.to_str().unwrap(), "create", size])
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
}

fn write_from_file(vhd: &Path, offset: &str, input: &Path) -> Output {
    img(&[vhd.to_str().unwrap(), "write", "--offset", offset])
        .stdin(std::fs::File::open(input).unwrap())
        .output()
        .unwrap()
}

#[test]
fn write_refuses_an_input_past_the_virtual_disk_by_its_length() {
    let dir = scratch("past-end");
    let vhd = dir.join("small.vhd");
    let input = dir.join("huge.bin");
    create_dynamic(&vhd, "16M");
    let before = std::fs::read(&vhd).unwrap();

    // Twice the disk. Not sparse-sized: Windows allocates what set_len
    // asks for, so this stays small enough to exist on any runner.
    std::fs::File::create(&input)
        .unwrap()
        .set_len(32 << 20)
        .unwrap();
    let wrote = write_from_file(&vhd, "0", &input);
    let stderr = String::from_utf8_lossy(&wrote.stderr);
    assert_eq!(
        wrote.status.code(),
        Some(1),
        "a 32 MiB input was accepted: {stderr}"
    );
    assert!(
        stderr.contains("run past"),
        "refused, but not by its length: {stderr}"
    );
    assert!(
        std::fs::read(&vhd).unwrap() == before,
        "the refused write changed the image"
    );

    // And an input that fits still writes.
    std::fs::write(&input, b"fits").unwrap();
    let wrote = write_from_file(&vhd, "512", &input);
    assert!(
        wrote.status.success(),
        "{}",
        String::from_utf8_lossy(&wrote.stderr)
    );
    let r = vhd::VhdReader::open(&vhd).unwrap();
    let mut back = [0u8; 4];
    r.read_at(512, &mut back).unwrap();
    assert_eq!(&back, b"fits");
    drop(r);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Input through a pipe has no length to check first; it is read no
/// further than one byte past what fits, and refused before any is written.
#[test]
fn write_refuses_a_piped_input_past_the_virtual_disk_before_writing() {
    let dir = scratch("pipe-past-end");
    let vhd = dir.join("small.vhd");
    create_dynamic(&vhd, "1M");
    let before = std::fs::read(&vhd).unwrap();

    let mut child = img(&[vhd.to_str().unwrap(), "write", "--offset", "4096"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    // More than fits; the tool may stop reading before all of it is sent.
    let _ = stdin.write_all(&vec![0xAB; 2 << 20]);
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "an oversized pipe was accepted: {stderr}"
    );
    assert!(stderr.contains("more than"), "{stderr}");
    assert!(
        std::fs::read(&vhd).unwrap() == before,
        "the refused write changed the image"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// An image given as its own input is refused, and left as it was.
#[cfg(unix)]
#[test]
fn write_refuses_the_image_as_its_own_input() {
    let dir = scratch("self");
    let vhd = dir.join("self.vhd");
    create_dynamic(&vhd, "64M");
    let before = std::fs::read(&vhd).unwrap();

    // Through a different spelling of the same path, too.
    let spelled = dir.join(".").join("self.vhd");
    let wrote = write_from_file(&vhd, "0", &spelled);
    let stderr = String::from_utf8_lossy(&wrote.stderr);
    assert!(!wrote.status.success(), "an image was written into itself");
    assert!(stderr.contains("is the image being written"), "{stderr}");
    assert!(
        std::fs::read(&vhd).unwrap() == before,
        "the refused write changed the image"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
