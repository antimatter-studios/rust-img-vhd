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

### H2 — every constant in the CHS geometry ladder is a bare literal — **fixable, not yet done**

Grouped with H7/M4/M5 as one naming change.

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

### H7 — footer and dynamic-header offsets re-typed as literals in five places — **fixable, not yet done**

Grouped with H2/M4/M5.

---

## Medium

### M10 — three C-ABI helpers repeat one wrapper, two naming the wrong function on panic — **fixed**

`open_path` serves both `vhd_open` and `vhd_open_rw`; `open_on_device` serves
both `vhd_open_on_device` and `vhd_open_rw_on_device`. Each reported the
read-only name whichever was called, so **a crash report pointed at a function
the program may never have invoked**. The entry name is now threaded through.

The 25-line duplication itself is left — see below.

### M1, M2, M3, M4, M5 — duplication and unnamed wire values — **fixable, not yet done**

`read_u32`/`read_u64` byte-identical in two modules; `compute_checksum` twice
with a different skip window; the sector-bitmap arithmetic open-coded twice
*with opposite bounds discipline*; `SECTOR_SIZE` named in one module and
open-coded in three; the disk-type wire values 2/3/4 in four places.

M3 is the one to do first of these — two copies of the same arithmetic
disagreeing about bounds checking is how one of them ends up wrong.

### M6, M7, M8, M12 — structure and naming — **needs your decision**

`open_inner` at 90 lines doing seven jobs; three parallel `Option` fields that
must agree, guarded six times; `writable` the field and `writable()` the method
meaning different things; `next_alloc_off` encoding a file-layout assumption
nothing verifies. Each is a real observation and each fix changes a shape rather
than correcting a defect.

M8 is worth your attention: a field and a method with the same name and
different meanings is a trap, but renaming either touches the public surface.

### M9 — `chs_for_size`'s doc describes an input the function does not take — **fixable, not yet done**

Grouped with the naming change, since the same doc block covers the ladder
constants H2 is about.

### M11 — `open_path` dereferences a raw pointer but is not an `unsafe fn` — **needs your decision**

Correct, and the fix is a signature change with `unsafe` semantics attached —
worth doing deliberately rather than in a batch.

### M13 — `tmp_path` written three times, only two clean up — **fixable, not yet done**

Test hygiene; a panicking assertion leaves a file behind in the third.

---

## Verification

**56 tests pass, up from 55.** `chore lint` clean. The new test is the only
behavioural change: an image whose BAT cannot fit in it is now refused at open.
