//! `vhd_tool write` refuses an input that does not fit before reading it.
//!
//! It used to read the whole input into memory and then let `write_at`
//! report it out of bounds, so a large input -- or an image selected as
//! its own input -- cost its full size in memory to end in an error.

use std::process::Command;

#[test]
fn write_refuses_an_input_past_the_virtual_disk_by_its_length() {
    let dir = std::env::temp_dir().join(format!("vhd-tool-write-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let vhd = dir.join("small.vhd");
    let input = dir.join("huge.bin");
    let tool = env!("CARGO_BIN_EXE_vhd_tool");

    let made = Command::new(tool)
        .args(["create-dynamic", vhd.to_str().unwrap(), "16777216"])
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );

    // Sparse: 64 GiB of length and no bytes on disk, so reading it whole
    // is what would cost, not creating it.
    std::fs::File::create(&input)
        .unwrap()
        .set_len(64 << 30)
        .unwrap();
    let wrote = Command::new(tool)
        .args(["write", vhd.to_str().unwrap(), "0", input.to_str().unwrap()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&wrote.stderr);
    assert!(!wrote.status.success(), "a 64 GiB input was accepted");
    assert!(
        stderr.contains("run past"),
        "refused, but not by its length: {stderr}"
    );

    // And an input that fits still writes.
    std::fs::write(&input, b"fits").unwrap();
    let wrote = Command::new(tool)
        .args([
            "write",
            vhd.to_str().unwrap(),
            "512",
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();
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
