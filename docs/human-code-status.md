# Human-code findings — status

Tracks every **High** and **Medium** finding from
[`human-code-report-2026-08-28.md`](human-code-report-2026-08-28.md). The report
predates the work; this is the current position. Updated 2026-08-30.

**28 findings** — 7 High, 13 Medium, 8 Low. This covers the 20 High and Medium.

| | High | Medium |
|---|---|---|
| Fixed | 5 | 1 |
| Left for a human decision | 0 | 6 |
| Fixable, not yet done | 2 | 6 |

---

## The one that mattered

**H6 — the BAT was allocated from a completely unvalidated on-disk `u32`.**

```rust
let bat_entries = dyn_hdr.max_table_entries as usize;
let mut bat_bytes = vec![0u8; bat_entries * 4];
```

`max_table_entries` is a `u32` read straight off disk. Unbounded, a corrupt or
hostile image asks for **up to 16 GiB before a single byte of the BAT is read**
— a header is all it takes. The field's own comment claimed the BAT was "always
small", which was an assertion about a number nothing checked.

Two bounds now, and both are the image's own arithmetic rather than a figure
invented here: the table must describe at least the declared virtual size, and
it must fit inside the file it lives in.

`absurd_max_table_entries_is_refused_before_allocating` rewrites the field *and
its checksum*, so the size bound is what rejects the image rather than the
checksum incidentally catching it. Against the unbounded code, `open` succeeds.

---

## High

### H1 — the qemu cross-validation checked CHS against a second copy of itself — **fixed earlier**

[#23](https://github.com/antimatter-studios/rust-img-vhd/pull/23).

### H2 — every constant in the CHS geometry ladder is a bare literal — **fixed**

`format::chs` names all eight: the three field ceilings, the two derived sector
ceilings, the three sectors-per-track rungs, the minimum head count and the
legacy cylinders-per-head budget. The ladder now reads as the specification's
pseudo-code rather than as arithmetic on bare digits.

The two derived ceilings are checked with `const _: () = assert!(…)` rather than
a test — including that the ladder's last rung stays below saturation, without
which the else-arm is unreachable. A build failure is a better place to learn
that than a test run.

### H3 — `open_parent`'s doc described behaviour the function does not have — **fixed**

The doc said locators first, sibling lookup as a fallback. The function reads
the locators, explicitly discards them, and resolves `parent_unicode_name`
against the child's directory — as an inline comment a few lines down correctly
said.

Two contradicting comments, and the reader meets the wrong one first. It now
says what happens, including the consequence: a parent that is not a sibling is
not found.

### H4 — `allocate_block`'s tail rollback restored the wrong value — **fixed earlier**

[#23](https://github.com/antimatter-studios/rust-img-vhd/pull/23), and
thoroughly: the allocator now holds the BAT lock, *reads* the tail rather than
reserving it, and commits after zero-init. The comment there explaining why a
leak is preferable to aliasing is worth reading.

### H5 — `write_sparse` released the BAT lock before deciding to allocate — **fixed earlier**

Same PR.

### H6 — the BAT allocated from an unvalidated `u32` — **fixed**, see above.

### H7 — footer and dynamic-header offsets re-typed as literals in five places — **fixed, but not the way it was written up**

`format::footer_offsets` and `format::dynamic_offsets` name every field, and the
parser and the builder now index them. Those two *must* agree, so they share one
definition.

**The test fixtures were deliberately left alone**, which is the opposite of
what the report proposed, and the measurement is why. Every fixture writes
literal offsets, so a wrong constant shows up as a failure: moving `DISK_TYPE`
by one byte fails 37 tests, shifting the footer's checksum window 22, moving the
dynamic header's `BLOCK_SIZE` 17. Rewriting the fixtures against the constants
would take all three to zero and leave a crate that agrees with itself and with
nothing else.

So the fixtures are the second opinion, and `format.rs` says so, in the module
docs and in `layout_matches_the_published_specification` — a deliberate third
copy of the table, transcribed from the specification, plus
`no_field_overlaps_its_neighbour`, which catches a field widened without its
neighbour moving (every individual assertion still passes in that case).

---

## Medium

### M10 — three C-ABI helpers repeat one wrapper, two naming the wrong function on panic — **fixed**

`open_path` serves both `vhd_open` and `vhd_open_rw`; `open_on_device` serves
both `vhd_open_on_device` and `vhd_open_rw_on_device`. Each reported the
read-only name whichever was called, so **a crash report pointed at a function
the program may never have invoked**. The entry name is now threaded through.

The 25-line duplication itself is left — see below.

### M3 — the sector-bitmap arithmetic, open-coded twice with opposite bounds discipline — **fixed**

```rust
// read side                                  // write side
let bit_byte  = (n / 8) as usize;             let bit_byte  = (n / 8) as usize;
let bit_in_byte = 7 - (n % 8) as u8;          let bit_in_byte = 7 - (n % 8) as u8;
let bit_set = (bitmap[bit_byte] …             if bit_byte >= bitmap.len() {
                                                  return Err(Error::Corrupt(…));
                                              }
```

Both are safe, and the asymmetry reads as though one of them is a bug. Deciding
which meant reconstructing the sizing argument from `dynamic.rs` — that
`bitmap_size_bytes` is one bit per sector of the block rounded up to a whole
sector, so a bitmap always covers every sector its block can hold.

`bitmap_get` / `bitmap_set` state the ordering once and give the bounds question
one answer. **Neither checks**, and the doc says why: an index past the end would
mean a caller had asked about a sector outside the block, which is a bug here and
not corrupt input. The old `Error::Corrupt("bitmap index out of range")` named the
input as the fault, and the input has nothing to do with it.

Panicking beats returning `false` for the same reason: a silent `false` reads as
"this sector is a hole" and hands the caller zeroes for data that exists.

`7 - (n % 8)` is the format's MSB-first ordering — sector 0 is bit 7 of byte 0 —
and it is the one fact the two paths must agree on. **Getting it backwards is
invisible to a round trip through this crate**, since writer and reader would
agree with each other and disagree with every other VHD tool, so the test asserts
against literal bytes rather than against itself. A third test checks the sizing
invariant across four block sizes, which is the argument both helpers rest on.

Mutation-checked: `7 - (n % 8)` written as `n % 8` fails two of the three.

### M1, M2, M4, M5 — duplication and unnamed wire values — **fixed**

All four now live in `src/format.rs`:

- **M1** — one `read_u32` / `read_u64`, with the bounds question answered in the
  doc (a short slice is a bug in this crate, not corrupt input; every caller has
  already checked it has a whole footer or header). `footer_build` gained the
  matching `write_u32` / `write_u64`, so the builder and the parser now index the
  same constants and a field can only move in both at once.
- **M2** — one `ones_complement_checksum(bytes, size, skip)`. The length and the
  skip window were the only two differences between the copies, and they are now
  the two arguments; writing the loop twice made it look as though there might be
  a third.
- **M4** — one `SECTOR_SIZE`, with the doc saying what it is *not*: the block
  device's sector size. A VHD on a 4 KiB-sector disk still has 512-byte VHD
  sectors, because the number belongs to the file format.
- **M5** — `format::disk_type_wire::{FIXED, DYNAMIC, DIFFERENCING}`.

Mutation-checked, and detectability went **up** rather than down, because the
fixtures stayed literal: the shared `read_u32` flipped to little-endian fails 42
tests where the two old copies failed 37 and 19; the shared `DISK_TYPE` moved by
a byte fails 39 where the old copy failed 37.

### M6, M7, M8, M12 — structure and naming — **needs your decision**

`open_inner` at 90 lines doing seven jobs; three parallel `Option` fields that
must agree, guarded six times; `writable` the field and `writable()` the method
meaning different things; `next_alloc_off` encoding a file-layout assumption
nothing verifies. Each is a real observation and each fix changes a shape rather
than correcting a defect.

M8 is worth your attention: a field and a method with the same name and
different meanings is a trap, but renaming either touches the public surface.

### M9 — `chs_for_size`'s doc describes an input the function does not take — **fixed**

The doc opens by stating the input is a byte count, that the specification's
pseudo-code starts from a sector count instead, and that the conversion is the
first line of the body — so the reader is not left comparing a doc about sectors
against a signature taking bytes. Its reference to the sector ceiling now names
`chs::MAX_ADDRESSABLE_SECTORS` rather than repeating `65535 * 16 * 255`.

### M11 — `open_path` dereferences a raw pointer but is not an `unsafe fn` — **needs your decision**

Correct, and the fix is a signature change with `unsafe` semantics attached —
worth doing deliberately rather than in a batch.

### M13 — `tmp_path` written three times, only two clean up — **fixed**

One `TempPath` and one `tmp_path_with(prefix, name, ext)` in `tests/common/mod.rs`,
used by all three suites.

The third copy — `synthetic.rs` — returned a bare `PathBuf` and removed the file
on the last line of each test, which is exactly the line a panicking assertion
never reaches. Every failing run leaked a fixture, and these images run to a few
MiB. `Drop` runs during unwinding, so the RAII type cleans up on the failing
path, which was the only path that ever leaked. 25 now-redundant `remove_file`
calls came out with it.

---

## Verification

**61 tests pass, up from 56.** (Three added; one runtime test became a set of compile-time assertions, which no longer counts as a test but fails the build instead.) `chore lint` clean, including
`--all-features`. Two behavioural changes across the whole sweep: an image whose
BAT cannot fit in it is now refused at open, and a panicking synthetic test no
longer leaves its fixture in the temp directory.
