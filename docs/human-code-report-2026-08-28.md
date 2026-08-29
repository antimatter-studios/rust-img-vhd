# Human-code report — am-img-vhd

**This document is analysis only. No code was changed. No files other than this one were
created or modified, nothing was committed, and no branch was made.** Phases 0 (Understand),
1 (Scan and Triage) and 3 (Report) of the `human-code` skill were run; Phase 2 (the dev-loop
implementation pass) was deliberately not started, pending your read of this document.

---

## Header

| | |
|---|---|
| **Date** | 2026-08-28 |
| **Crate** | `am-img-vhd` 0.3.2 (`[lib] name = "vhd"`), `/Volumes/sdcard256gb/projects/rust-img-vhd` |
| **Scope** | Full crate — `src/` (6 modules + 1 bin), `tests/` (3 suites), `include/vhd.h`, `chores.yml` |
| **Lines scanned** | 3,233 Rust (1,672 `src/`, 1,295 `tests/`, 118 `src/bin/`) |
| **Items found** | **28** |
| **Items fixed** | **0** (report-only run) |
| **Items skipped** | **0** (nothing triaged away; the full list is below) |
| **Baseline** | 50 tests passing, 0 failing (`cargo test`); `cargo clippy --all-targets -- -D warnings` clean |

**Counts by severity: 7 High · 13 Medium · 8 Low.**

The crate is in good shape structurally — small modules, honest error type, no `unsafe`
outside the C ABI, clippy-clean at `-D warnings`. What it lacks is *naming*. The VHD spec's
numbers are almost all present in the source as bare literals, and the two places where the
code most needs a reader's trust — the CHS geometry ladder and the dynamic-write allocator —
are the two places carrying the least explanation and, in the allocator's case, comments that
assert invariants the code below them breaks.

---

## Findings

### High

---

#### H1 — the qemu cross-validation test checks the CHS algorithm against a second copy of itself

- **Files:** `src/footer_build.rs:104-147` (production `chs_for_size`), `tests/qemu_validation.rs:155-190` (`chs_derived_size`)
- **Category:** Duplicated code / test that cannot fail
- **Severity:** High
- **Test coverage:** `qemu_extracts_bytes_from_vhd_we_created` (`tests/qemu_validation.rs:262`), `qemu_reports_our_fixed_vhd_virtual_size` (`:301`). Both are behind the `qemu-validation` feature and do **not** run in a default `cargo test`.

`chs_derived_size` is a line-for-line reimplementation of `chs_for_size` — same ceiling clamp,
same `17 → 31 → 63` spt ladder, same `div_ceil(1024)` head calculation, same `heads == 0`
guard — differing only in that the maxed-out branch is inlined as `(total_sectors / 255, 16, 255)`
and that it folds `C*H*S*512` back into bytes at the end.

The point of the qemu suite is to check our footer against an *independent* implementation. For
geometry it checks our footer against our own algorithm, written twice. If the spec pseudo-code
was transcribed wrong, both copies are wrong identically and the test still passes. The only
genuinely independent number in the whole geometry assertion is the hard-coded `4_177_920` at
`tests/qemu_validation.rs:314`, which pins exactly one input size (8192 sectors).

The copy exists because `chs_for_size` is private. Making it `pub(crate)` and reachable from the
integration test, then replacing `chs_derived_size` with a small table of spec-sourced
`(size, C, H, S)` triples, converts a tautology into a real oracle.

---

#### H2 — every constant in the CHS geometry ladder is a bare literal

- **File:** `src/footer_build.rs:104-147`
- **Category:** Magic numbers
- **Severity:** High
- **Test coverage:** `chs_geometry_is_within_spec_bounds_for_typical_sizes` (`src/footer_build.rs:231`) and `chs_geometry_clamps_at_and_beyond_the_spec_ceiling` (`:241`). The first only asserts `c > 0 && 0 < h <= 16 && s > 0` — it would pass on a badly wrong ladder. The second is the sharper of the two, but covers only the ceiling.

Present, unnamed, in a 43-line function:

| Literal | Meaning | Occurrences |
|---|---|---|
| `65535` | max cylinders | `:107`, `:108`, `:111` + `:246`, `:264` (tests) + 2 in the qemu copy |
| `16` | max heads — *and separately* the maxed-out head count | `:107`, `:108`, `:111`, `:115`, `:124`, `:126`, `:131` |
| `255` | max sectors-per-track | `:107`, `:108`, `:113` |
| `63` | legacy spt — *and* part of the large-disk threshold | `:111`, `:130` |
| `17`, `31` | the spt ladder's first two rungs | `:118`, `:125` |
| `1024` | cylinders-per-head limit | `:120`, `:124`, `:129` |
| `4` | minimum heads | `:121`, `:122` |
| `512` | sector size | `:106` |

`65535u64 * 16 * 255` is spelled out twice inside the function alone (`:107`, `:108`) and four
more times across the tests. A reader cannot tell the `16` at `:107` (max heads, part of the
sector ceiling) from the `16` at `:115` (the head count actually assigned) from the `16` at
`:124` (the head-count sanity bound) — they mean three different things and are typed identically.
Likewise `63` at `:111` is a threshold multiplicand while `63` at `:130` is an spt value.

This is the file the brief singles out as fiddly, and it is exactly where naming buys the most.
Suggested home: a `mod chs` const group in `footer_build.rs` — `MAX_CYLINDERS`, `MAX_HEADS`,
`MAX_SECTORS_PER_TRACK`, `LEGACY_SECTORS_PER_TRACK`, `CYLINDERS_PER_HEAD_LIMIT`, `MIN_HEADS`,
`SPT_LADDER: [u32; 3] = [17, 31, 63]`, plus the two derived thresholds
`MAX_TOTAL_SECTORS = MAX_CYLINDERS * MAX_HEADS * MAX_SECTORS_PER_TRACK` and
`LARGE_DISK_THRESHOLD_SECTORS = MAX_CYLINDERS * MAX_HEADS * LEGACY_SECTORS_PER_TRACK` — so the two
`65535 * 16 * …` products each get written once and named for what they gate.

---

#### H3 — `open_parent`'s doc comment describes behaviour the function does not have

- **File:** `src/reader.rs:655-701` — the claim at `:656-657`, the contradiction at `:667-669`
- **Category:** Comment that lies
- **Severity:** High
- **Test coverage:** `differencing_falls_through_to_parent_for_unallocated` (`tests/synthetic.rs:250`) exercises the sibling-name path only. No test covers locator resolution, because there isn't any.

The doc comment says:

> Tries the W2ku/W2ru relative-path locators first, then falls back to a sibling lookup using
> `parent_unicode_name`.

The function's first statement is `let _ = dyn_hdr.parent_locators; // explicitly acknowledged`
(`:669`) and it never touches them again. Three lines below the doc comment, an inline comment
says the opposite and is correct: *"For now: just use the parent_unicode_name + child's
directory. Locator-data resolution can land in a follow-up."*

So the function carries two comments that contradict each other, and the one a reader sees first
— the doc comment, the one that shows up in `cargo doc` and on hover — is the false one. Anyone
debugging a differencing image that resolves through a locator will read the doc, conclude
locators are consulted, and go looking in the wrong place.

The dead `let _ = …` binding at `:669` is a second, smaller smell: it exists to silence nothing
(the field is `pub` on a `pub` struct, so it is not dead-code-warned) and reads as a placeholder
for work that the doc comment claims is already done.

---

#### H4 — `allocate_block`'s tail rollback restores the wrong value

- **File:** `src/reader.rs:583-605`
- **Category:** Dense logic contradicting its own comment
- **Severity:** High
- **Test coverage:** **None.** `dynamic_write_allocates_fresh_block` (`tests/synthetic.rs:684`) and `dynamic_write_spanning_block_boundary_allocates_both` (`:726`) cover the success path single-threaded. There is no test for a failing device write, and no concurrent test anywhere in the suite.

The reservation, at `:583-588`:

```rust
let new_block_off = {
    let mut tail = self.next_alloc_off.lock().unwrap();
    let cur = tail.ok_or(Error::Corrupt("allocate but no tail offset (fixed?)"))?;
    *tail = Some(cur + block_total);
    cur
};
```

The rollback, at `:595-600` and again at `:601-605`:

```rust
if let Err(e) = self.dev_write(new_block_off, &zeros) {
    // Roll back the tail reservation — the device refused.
    let mut tail = self.next_alloc_off.lock().unwrap();
    *tail = Some(new_block_off);
    return Err(e);
}
```

`new_block_off` *is* `cur`. So the rollback does not undo this thread's advance — it assigns the
tail an absolute value, discarding whatever any other thread reserved in the interim. `VhdReader`
is `Sync`, `write_at` takes `&self`, and `capi.rs:59` / `capi.rs:122` hand the reader out inside
an `Arc`, so two threads can be inside `allocate_block` simultaneously. When the loser rolls back,
it rewinds the tail over the winner's live reservation and the next allocation writes a block on
top of one already in use.

The comment two lines above the reservation states precisely the invariant the rollback breaks:
*"Reserve the next tail offset under lock so concurrent allocations don't collide."*

Secondary: only step 1 rolls back at all. Steps 2 (`:610-611`) and 3 (`:623-624`) propagate with
`?` and leave the tail permanently advanced past a block that was never published to the BAT.
The function's own doc comment (`:561-571`) walks through the three-step crash-safety order in
detail and says nothing about which steps unwind.

---

#### H5 — `write_sparse` releases the BAT lock before deciding whether to allocate

- **File:** `src/reader.rs:521-538`
- **Category:** Dense logic / comment explaining WHAT instead of WHY
- **Severity:** High
- **Test coverage:** **None** for the concurrent case; the single-threaded path is covered by the three `dynamic_write_*` tests.

```rust
// Fast-path: read current BAT entry under lock, drop lock
// before touching the device.
let bat_entry = { /* lock, read, unlock */ };

let block_host_off = if bat_entry == BAT_UNALLOCATED {
    self.allocate_block(block_idx)?
} else {
    bat_entry as u64 * SECTOR_SIZE
};
```

Two threads writing to different offsets *within the same unallocated block* both read
`BAT_UNALLOCATED`, both call `allocate_block`, and both allocate. One BAT entry wins; the other
block is orphaned in the file with the loser's payload in it, and the loser's write is silently
lost.

The comment presents the lock release as the design ("Fast-path… drop lock before touching the
device") rather than as a constraint, so a reader is told *what* the code does and steered away
from asking whether it is safe. Nothing anywhere in the crate records that single-writer use is
assumed — and the type opts into `Sync`, exposes `write_at(&self)`, and is distributed behind
`Arc`, all of which advertise the opposite.

Whichever way this resolves (hold the lock across allocation, or document single-writer and say
so at the type), the comment needs to carry the reasoning rather than the mechanics.

---

#### H6 — the BAT is allocated from a completely unvalidated on-disk `u32`

- **File:** `src/reader.rs:157-165`; the false invariant at `src/reader.rs:57`
- **Category:** Missing validation / comment asserting an unenforced invariant
- **Severity:** High
- **Test coverage:** **None.** `tests/corruption.rs` corrupts the footer only (cookie, checksum, disk type) and never the dynamic header. `src/dynamic.rs`'s unit tests validate `block_size` but never `max_table_entries`.

```rust
let bat_entries = dyn_hdr.max_table_entries as usize;
let mut bat_bytes = vec![0u8; bat_entries * 4];
```

`max_table_entries` comes straight off disk with no bound. A corrupt or hostile image can request
a 16 GiB allocation before a single byte of the BAT is read. The field comment at `:57` says the
BAT is *"always small — `max_table_entries * 4` bytes"* — an assertion of an invariant that
nothing in the crate checks.

The check that belongs here is exactly the "header field combination is internally inconsistent"
case `Error::Corrupt` was created for: `max_table_entries` should be
`virtual_size.div_ceil(block_size)`, and `table_offset + max_table_entries * 4` must fall inside
the device. `DynamicHeader::parse` already does this style of validation for `block_size`
(`src/dynamic.rs:81-86`), so the pattern and the error variant are both already established —
this field just missed the pass.

---

#### H7 — the footer and dynamic-header field offsets are re-typed as literals in five places

- **Files:** `src/footer.rs:73-91` (parse) · `src/footer_build.rs:41-72` (build) · `src/footer.rs:148-176` and `src/dynamic.rs:178-204` (unit-test fixtures) · `tests/synthetic.rs:40-57` and `:137-145` (integration fixtures) · `tests/corruption.rs:93`, `:116-118` (corruption fixtures)
- **Category:** Magic numbers, duplicated code
- **Severity:** High
- **Test coverage:** Well covered *behaviourally* — 50 tests exercise these paths. But every one of those tests re-encodes the offsets itself, so the tests cannot catch an offset error; they'd have to make the same error to compile against it.

The 512-byte footer layout is documented once, beautifully, at `src/footer.rs:6-25`. It is then
re-encoded as raw slice indices in five separate places, none of which references that doc.

The checksum window is the clearest case. `64..68` appears at:

- `src/footer.rs:73` — `read_u32(bytes, 64)`
- `src/footer.rs:110` — `if (64..68).contains(&i)`
- `src/footer.rs:156` and `:176` — test fixtures
- `src/footer_build.rs:72` — `f[64..68].copy_from_slice(&cs.to_be_bytes())`
- `tests/synthetic.rs:55` — fixture
- `tests/corruption.rs:118` — fixture

Seven hand-written copies of one offset. The dynamic header's `36..40` has the same spread across
`src/dynamic.rs:65`, `:134`, `:185`, `:204`, `:272` and `tests/synthetic.rs:144`. `disk_type` at
60 appears in `footer.rs:88`, `footer_build.rs:63`, `footer.rs:154`/`:173`, `synthetic.rs:54`,
`corruption.rs:116`.

Change one offset and nothing makes the other six follow — the parser and the builder would
silently disagree, and the tests, having their own third copy, would keep passing or fail in a way
that points at the wrong file. Suggested home: a `mod offsets` const group per header
(`footer::offsets::{COOKIE, FEATURES, DATA_OFFSET, CURRENT_SIZE, DISK_TYPE, CHECKSUM, UNIQUE_ID, …}`),
used by parse, build **and** the fixtures, so the doc comment's table and the code are the same
artifact.

---

### Medium

---

#### M1 — `read_u32` / `read_u64` are byte-identical in two modules

- **Files:** `src/footer.rs:118-133`, `src/dynamic.rs:154-169`
- **Category:** Duplicated code
- **Severity:** Medium
- **Test coverage:** Indirect — every parse test exercises both copies.

The same eight-line big-endian decoders, character for character. Two instances is technically
under the skill's three-instance extraction bar, but a third copy appears the moment parent-locator
data (`src/dynamic.rs:43-44`, currently unparsed) grows a decoder. A crate-private `be` module —
or just `u32::from_be_bytes(bytes[off..off+4].try_into()?)` inline — removes both.

---

#### M2 — `compute_checksum` is the same algorithm twice with a different skip window

- **Files:** `src/footer.rs:107-116`, `src/dynamic.rs:131-140`
- **Category:** Duplicated code
- **Severity:** Medium
- **Test coverage:** `checksum_round_trip_for_minimal_fixed_footer` (`footer.rs:146`), `rejects_checksum_mismatch` (`footer.rs:200`), `checksum_round_trip_for_minimal_dynamic_header` (`dynamic.rs:176`), `rejects_checksum_mismatch` (`dynamic.rs:225`).

Identical loops. The only differences are the length (`FOOTER_SIZE` vs `DYN_HEADER_SIZE`) and the
excluded range (`64..68` vs `36..40`) — and `src/dynamic.rs:129-130` says so in a comment: *"Same
algorithm as the footer."* A shared `ones_complement_sum(bytes, len, checksum_field: Range<usize>)`
with both existing `pub fn`s kept as thin wrappers preserves the public API (both are used by
`tests/synthetic.rs:15-18`) while stating the shared rule once.

---

#### M3 — the sector-bitmap bit arithmetic is open-coded twice, with opposite bounds discipline

- **Files:** `src/reader.rs:430-432` (read side), `src/reader.rs:641-648` (write side)
- **Category:** Dense expressions / duplicated code
- **Severity:** Medium
- **Test coverage:** `dynamic_partial_bitmap_zero_fills_unset_sectors` (`tests/synthetic.rs:220`) covers the read side; `dynamic_write_into_existing_block_round_trips` (`:649`) covers the write side. Neither covers an out-of-range bit index.

Read side:

```rust
let bit_byte = (sector_in_block / 8) as usize;
let bit_in_byte = 7 - (sector_in_block % 8) as u8;
let bit_set = (bitmap[bit_byte] >> bit_in_byte) & 1 == 1;
```

Write side computes the identical `bit_byte` / `bit_in_byte` pair — and then does check the bound:

```rust
if bit_byte >= bitmap.len() {
    return Err(Error::Corrupt("bitmap index out of range"));
}
```

Both are safe today, because `bitmap_size_bytes()` is derived from `block_size` and so always
covers every sector in the block. But nothing at either site says that, and the asymmetry reads
like one of the two is a bug — a reader has to reconstruct the sizing argument from
`src/dynamic.rs:120-126` to be sure which.

The `7 - (n % 8)` is the spec's MSB-first sector ordering. It deserves a name far more than it
deserves a comment: a `bitmap_get(&[u8], sector) -> bool` / `bitmap_set(&mut [u8], sector)` pair
states the ordering once, makes the two sites provably consistent, and gives the bounds question
a single answer.

---

#### M4 — `SECTOR_SIZE` is named in exactly one module and open-coded in the other three

- **Files:** `src/reader.rs:37` (the named constant) vs `src/dynamic.rs:84`, `:92`, `:121`, `:125` and `src/footer_build.rs:106`
- **Category:** Magic numbers
- **Severity:** Medium
- **Test coverage:** `bitmap_size_rounds_up_to_a_sector` (`src/dynamic.rs:249`) covers the sizing arithmetic across three block sizes; the rest is incidental.

`reader.rs` — the one module that reaches for the sector size most, and the one that named it. The
other modules did not. `bitmap_size_bytes` is the worst of them:

```rust
let sectors = self.block_size as u64 / 512;
let bits = sectors;
let bytes = bits.div_ceil(8);
bytes.div_ceil(512) * 512
```

Four unexplained numbers in four lines, of which two are the sector size and one is bits-per-byte.
`chs_for_size` opens with `size_bytes / 512`, and `DynamicHeader::parse` rejects
`block_size < 512` at `:84` with the number restated in the error string at `:85`.

Suggested home: `SECTOR_SIZE` promoted out of `reader.rs` to a crate-level `constants` module (or
into `footer`, which already owns `FOOTER_SIZE`), plus `BITS_PER_BYTE`. Worth a line of comment
where it lands: `FOOTER_SIZE == SECTOR_SIZE` is not a coincidence (the spec puts the footer in one
sector), and `reader.rs:186` and `:622` quietly rely on the two being interchangeable.

---

#### M5 — the disk-type wire values 2/3/4 are written in four places

- **Files:** `src/footer.rs:36-40` (`#[repr(u32)]` discriminants), `src/footer.rs:44-47` (`from_u32`'s match), `src/footer_build.rs:10` (`DISK_TYPE_FIXED: u32 = 2`), `tests/synthetic.rs:49-53` (a hand-written match back to `u32`)
- **Category:** Magic numbers / duplicated code
- **Severity:** Medium
- **Test coverage:** `disk_type_from_u32_accepts_known_and_rejects_unknown` (`src/footer.rs:181`) is thorough on the decode direction. The encode direction has no test — because there is no encode function; both call sites hand-roll it.

`#[repr(u32)]` already fixes the wire values. `from_u32` restates them, `footer_build.rs` restates
`2` a third time under its own name, and the integration fixture writes a fourth match converting
`DiskType` back to `u32`. A `DiskType::to_u32(self)` (or just letting `as u32` be the documented
encode path) lets `build_fixed_footer:63` write `DiskType::Fixed as u32` and lets
`tests/synthetic.rs` delete its match entirely.

---

#### M6 — `open_inner` is a 90-line constructor doing seven jobs

- **File:** `src/reader.rs:121-210`
- **Category:** God function
- **Severity:** Medium
- **Test coverage:** Heavily exercised — every one of the 50 tests that opens an image runs through it — but only at the whole-function level. No sub-step is independently testable.

Depth guard → device-size guard → footer read+parse → dynamic-header read+parse → BAT read+decode
→ bitmap sizing → parent open → tail-offset derivation → struct assembly. The tell is at `:144`:

```rust
let (dynamic, bat, bitmap_size, parent, next_alloc_off) = match footer.disk_type {
    DiskType::Fixed => (None, None, 0, None, None),
    DiskType::Dynamic | DiskType::Differencing => { /* 50 lines */ }
};
```

A five-tuple threaded out of a `match` to carry state across two arms — those five values are one
thing (the sparse-disk state) pretending to be five, which is the same observation as M7 seen from
the constructor's side. Extracting the `Dynamic | Differencing` arm into
`fn open_sparse_state(dev, footer, depth, owning_path) -> Result<SparseState>` collapses the tuple,
halves the function, and gives the BAT-validation check from H6 an obvious home.

---

#### M7 — three parallel `Option` fields that must agree, guarded six times over

- **File:** `src/reader.rs:53-72` (`dynamic`, `bat`, `next_alloc_off`)
- **Category:** Speculative/defensive code for scenarios that can't happen
- **Severity:** Medium
- **Test coverage:** The `Error::Corrupt` arms these guards produce are unreachable and therefore untested — and untestable without constructing an invalid `VhdReader`, which the module makes impossible.

All three fields are `None` exactly when the disk is fixed and `Some` exactly when it isn't.
`open_inner` guarantees it. The type does not express it, so every sparse code path opens with a
defensive unwrap for a state that cannot occur:

- `:376` — `.ok_or(Error::Corrupt("sparse read but no dynamic header"))`
- `:399` — `.ok_or(Error::Corrupt("sparse read but no BAT"))`
- `:505` — `.ok_or(Error::Corrupt("sparse write but no dynamic header"))`
- `:527` — `.ok_or(Error::Corrupt("sparse write but no BAT"))`
- `:585` — `.ok_or(Error::Corrupt("allocate but no tail offset (fixed?)"))`
- `:614` — `if let Some(bat) = bat_guard.as_mut()` — which *silently does nothing* if the impossible happens, unlike the five above

Six guards, five distinct error strings, one silent no-op, all for one invariant. A single
`sparse: Option<SparseState { header: DynamicHeader, bat: Mutex<Vec<u32>>, bitmap_size: u64, next_alloc: Mutex<u64> }>`
deletes all six and makes "fixed disks have no sparse state" a fact the compiler enforces rather
than a fact six error strings speculate about.

---

#### M8 — `writable` the field and `writable()` the method mean different things

- **File:** `src/reader.rs:51` (field), `:306-308` (method), `:349` (field used directly)
- **Category:** Misleading names
- **Severity:** Medium
- **Test coverage:** Good — `fixed_opened_read_only_is_not_writable` (`tests/synthetic.rs:430`), `fixed_opened_read_write_is_writable` (`:448`), `dynamic_opened_read_only_rejects_writes` (`:466`), `dynamic_opened_rw_is_writable` (`:481`), `differencing_is_not_writable` (`:494`).

The field means "the backing device was opened RW". The method means "the device was opened RW
**and** this subtype has a write path" — so a differencing VHD opened RW has `self.writable == true`
and `self.writable() == false`. The two spellings differ by two characters, and the crate uses both:

- `write_at:314` gates on `self.writable()` — correct, refuses differencing writes
- `flush_writes:349` gates on `self.writable` — correct too, but for a *different* reason (a
  differencing handle opened RW should still flush), which nothing states

Both call sites are right; the problem is that a reader must derive that from first principles
each time, because the names carry no distinction. `device_is_rw` for the field (or
`opened_rw`) against `writable()` for the method would make the two reads distinguishable
at a glance and turn `flush_writes`'s choice into a visible decision.

---

#### M9 — `chs_for_size`'s doc comment describes an input the function doesn't take

- **File:** `src/footer_build.rs:97-104`
- **Category:** Comment that lies
- **Severity:** Medium
- **Test coverage:** N/A (documentation).

> *Input: total sectors of the virtual disk (size_bytes / 512).*

The parameter is `size_bytes: u64`; the division to sectors happens on the first line of the body.
The doc is describing the spec's pseudo-code rather than this function, so a caller reading the
signature and a caller reading the doc get different answers about what to pass.

The same comment ends: *"The spec caps everything below the magic 65535\*16\*255 ceiling and
promises the result is 'as close as possible' to the requested size."* That is the spec's claim
about geometry, not a property a caller can rely on. `tests/qemu_validation.rs:148-154` documents
the actual behaviour far better than the function it describes — geometry rounds *down*, by 16,384
bytes at a requested 4 MiB. That paragraph belongs on `chs_for_size`, where a caller will find it.

---

#### M10 — the three C-ABI entry helpers repeat one 25-line wrapper, two of them naming the wrong function on panic

- **File:** `src/capi.rs:41-73` (`vhd_create_fixed`), `:101-136` (`open_on_device`), `:138-172` (`open_path`)
- **Category:** Duplicated code
- **Severity:** Medium
- **Test coverage:** **None.** `src/capi.rs` has zero tests; the C ABI is not exercised anywhere in `tests/`.

The identical shape three times: null check → `set_last_error` → `catch_unwind(AssertUnwindSafe(…))`
→ `CStr::from_ptr` → `to_str` → `set_last_error` on failure → `into_handle` / `null_mut` → outer
`match res` → `set_last_error("panic in …")`. Three instances clears the extraction bar.

Two of them then hard-code the wrong entry-point name in the panic message, because the helper is
shared but the string isn't:

- `capi.rs:168` reports `"panic in vhd_open"` — but `open_path` is also the body of `vhd_open_rw` (`:32`)
- `capi.rs:132` reports `"panic in vhd_open_on_device"` — but `open_on_device` is also the body of `vhd_open_rw_on_device` (`:98`)

A C caller who reads `fs_core_last_error_message()` after a panic in `vhd_open_rw` is told the
panic happened in `vhd_open`. Passing the entry-point name into the helper fixes both, and is the
natural thing to do while collapsing the three copies into one.

---

#### M11 — `open_path` dereferences a raw pointer but isn't an `unsafe fn`

- **File:** `src/capi.rs:138` vs `src/capi.rs:101`
- **Category:** Misleading signatures
- **Severity:** Medium
- **Test coverage:** None.

`open_on_device` is `unsafe fn`. `open_path` is a safe `fn` that does `unsafe { CStr::from_ptr(path) }`
inside its body (`:144`). Both are private helpers taking a caller-supplied pointer with identical
trust assumptions. The crate sets `#![deny(unsafe_op_in_unsafe_fn)]` (`src/lib.rs:19`), so the
inconsistency looks deliberate but is unexplained — and the safe signature says `open_path` can be
called with any `*const c_char`, which is exactly the thing that is not true.

---

#### M12 — `next_alloc_off` encodes a file-layout assumption that nothing verifies

- **File:** `src/reader.rs:182-186`
- **Category:** Speculative code / missing validation
- **Severity:** Medium
- **Test coverage:** The happy path only — the three `dynamic_write_*` tests all use fixtures built by `build_dynamic_vhd` / `build_dynamic_vhd_all_sparse` (`tests/synthetic.rs:122`, `:772`), which construct exactly the canonical layout. No test uses an image with trailing slack.

```rust
let next_alloc = dev_size.saturating_sub(FOOTER_SIZE as u64);
```

That is the next safe allocation point *if* the file ends exactly at "last block + footer". The
comment says so, honestly: *"assuming the device was sized to 'end of data + footer' — the
canonical layout."* But a dynamic VHD with any trailing slack — or one whose highest-offset block
is not the last thing before the footer — gets its first newly-allocated block written straight
over live data.

The BAT is fully parsed and in memory four lines earlier. `max(allocated BAT entry) * SECTOR_SIZE
+ bitmap_size + block_size` is a *computed* answer to the same question, and the two disagreeing
is itself a corruption signal worth reporting. Right now the assumption is load-bearing and only
a comment stands behind it.

---

#### M13 — `tmp_path` is written three times and only two of the three clean up

- **Files:** `tests/synthetic.rs:21-27`, `tests/corruption.rs:15-42`, `tests/qemu_validation.rs:52-79`
- **Category:** Duplicated code
- **Severity:** Medium
- **Test coverage:** N/A (test infrastructure).

`corruption.rs` and `qemu_validation.rs` carry byte-identical `TempPath` RAII wrappers — same
`Deref`, same `AsRef<Path>`, same `Drop`. `synthetic.rs` has the path *generator* without the
wrapper, so its 25 tests leak a `.vhd` fixture into `$TMPDIR` on every run, including several
7 KiB+ dynamic images and a 16 KiB fixed one.

`corruption.rs:24-25` states exactly why the wrapper exists — *"removes the backing file on drop
so a panicking assertion can't leak fixtures into the temp dir across CI runs"* — which makes
`synthetic.rs`'s omission read as an oversight rather than a choice. Three copies clears the
extraction bar; a `tests/common/mod.rs` holding one `TempPath` would fix the leak as a side effect.

---

### Low

---

#### L1 — `let bits = sectors;`

- **File:** `src/dynamic.rs:120-126`
- **Category:** Misleading names / dense expressions
- **Severity:** Low
- **Test coverage:** `bitmap_size_rounds_up_to_a_sector` (`src/dynamic.rs:249`).

A rebinding that changes nothing but the name — one sector is one bit, which is the actual insight
and is stated only in the doc comment above. Four lines to express
`(block_size / SECTOR_SIZE).div_ceil(BITS_PER_BYTE).div_ceil(SECTOR_SIZE) * SECTOR_SIZE`. Folds
into M4.

---

#### L2 — `read_sparse` allocates the bitmap buffer inside the block loop

- **File:** `src/reader.rs:418`
- **Category:** Dense logic
- **Severity:** Low
- **Test coverage:** All three `dynamic_*` read tests.

`let mut bitmap = vec![0u8; bitmap_size as usize];` sits inside the `while cursor < end` body, so a
read spanning N allocated blocks does N heap allocations of an identically-sized buffer. Hoisting
it above the loop is both faster and one less thing in a loop body that already carries a nested
loop and the bit arithmetic from M3.

---

#### L3 — two of the three clamps in `chs_for_size`'s return are dead

- **File:** `src/footer_build.rs:142-146`
- **Category:** Defensive code for scenarios that can't happen
- **Severity:** Low
- **Test coverage:** `chs_geometry_clamps_at_and_beyond_the_spec_ceiling` (`:241`) covers the one clamp that does work.

```rust
cylinders.min(u16::MAX as u64) as u16,
heads.min(u8::MAX as u32) as u8,
sectors_per_track.min(u8::MAX as u32) as u8,
```

`heads` is provably in `{4..=16}` and `sectors_per_track` in `{17, 31, 63, 255}` — neither can
exceed `u8::MAX`, so those two `.min()` calls can never fire. Only the `cylinders` clamp is load-
bearing (and only because of the ceiling clamp at `:107`). Reading three symmetrical clamps
suggests all three ranges are open questions; naming the constants (H2) makes it obvious that two
of them are not.

---

#### L4 — `hex_dump`'s short-line padding doesn't account for the mid-line gap

- **File:** `src/bin/vhd_tool.rs:90-117`
- **Category:** Dense logic / magic numbers
- **Severity:** Low
- **Test coverage:** **None.** `src/bin/vhd_tool.rs` has no tests.

The byte loop emits an extra space after `j == 7` (`:96-98`); the padding loop emits a flat
`"   "` per missing byte (`:101-103`) and does not reproduce that gap. A final line of fewer than
8 bytes therefore lands its ASCII column one space left of every full line above it. Cosmetic, but
it is the tool's only output format.

`16` (bytes per line) also appears four times in 27 lines — `chunks(16)`, `* 16`, `..16`, and
implicitly in the `"   "` width.

---

#### L5 — `vhd_tool`'s 4 MiB read cap is an unnamed literal

- **File:** `src/bin/vhd_tool.rs:62`
- **Category:** Magic numbers
- **Severity:** Low
- **Test coverage:** None.

`if len > 4 * 1024 * 1024` — the reason lives only in the error string beside it (`"len too large
(cap 4 MiB)"`), so the number and its justification are two separate things that can drift.

---

#### L6 — `include/vhd.h` tells consumers to link a file the build does not produce

- **Files:** `include/vhd.h:6` vs `chores.yml:30-31`, `:77`
- **Category:** Comment that lies
- **Severity:** Low
- **Test coverage:** None — nothing checks the header against the build.

The header says *"Link with libam\_img\_vhd.a"*. `[lib] name = "vhd"` (`Cargo.toml:11`), so cargo
emits `libvhd.a`, and `chores.yml:77` copies `lib{{.LIBNAME}}.a` = `libvhd.a`.

The comment immediately above the variable in `chores.yml:29-30` calls out this exact trap —
*"`[package].name` is am-img-vhd; `[lib].name` is vhd, so cargo writes libvhd.a. The header
mirrors the lib name, not the package name."* The one file that got it wrong is the one a C
consumer reads first.

---

#### L7 — `open_parent`'s last-resort fallback opens an image-supplied path verbatim

- **File:** `src/reader.rs:689-694`
- **Category:** Speculative code / missing validation
- **Severity:** Low
- **Test coverage:** None — no test reaches the fallback branch.

After the sibling lookup fails, the code tries `PathBuf::from(parent_name)` unchanged.
`parent_name` is UTF-16 decoded straight out of the image header (`src/dynamic.rs:92`), so a
crafted differencing VHD can name any absolute path on the host as its parent. The exposure is
narrow — the named file must parse as a valid VHD before anything is read through it, and the open
is read-only — but the fallback is undocumented, unconditional, and unmentioned in the function's
doc comment.

Separately, `candidate.exists()` followed by `FileDevice::open(&candidate)` (`:684-686`, and again
at `:691-693`) is a TOCTOU pair. Using the open result directly — `match FileDevice::open(..)
{ Ok(d) => …, Err(_) => fall through }` — is both simpler to read and race-free.

---

#### L8 — `create_fixed` bypasses the `BlockDevice` abstraction the rest of the module routes through

- **File:** `src/reader.rs:230-250`
- **Category:** Inconsistent abstraction level
- **Severity:** Low
- **Test coverage:** `create_fixed_round_trip_pattern` (`tests/synthetic.rs:356`), `create_fixed_partial_write_within_bounds` (`:380`), `create_fixed_rejects_unaligned_size` (`:536`), plus all five of `tests/corruption.rs`.

Every other write in the file goes through `dev_write` / `dev_flush` (`:364-370`), the module's
stated *"central place to lift `fs_core::Error` into `crate::Error`"*. `create_fixed` instead uses
`OpenOptions`, `seek`, `write_all`, `sync_data` directly, then drops the handle and reopens
(`:250-252`). The reopen is defensible and the comment explains it (*"so the returned reader walks
the same code as any other fixed VHD"*), but the mixed I/O style within one impl block makes a
reader check whether the difference is meaningful. It isn't — `set_len` for sparse allocation is
the only thing `BlockDevice` can't express, and saying so in a line of comment would settle it.

---

## What to fix first

Recommended order. Rationale: correctness-adjacent items first, then the naming work the brief
calls out, then the structural cleanups that get easier once the names exist.

**1. H3 and H6 — one small commit each, no behaviour change beyond H6's new guard.**
H3 is a comment edit: make the doc comment match the body and delete the dead `let _`. H6 adds the
`max_table_entries` sanity check next to the `block_size` checks that already exist in
`DynamicHeader::parse`, plus one corruption test. Both are self-contained, both remove a statement
the code doesn't back up, and neither depends on anything else on this list.

**2. H4 and H5 — decide the concurrency contract, then make the code and comments say it.**
These are one decision, not two. Either `VhdReader` supports concurrent writers (hold the BAT lock
across `allocate_block`, fix the rollback to restore the previous tail rather than assign an
absolute one, and unwind steps 2 and 3) or it doesn't (document single-writer at the type, and
rewrite the three comments that currently imply otherwise). Right now the comments promise the
first and the code implements neither. This needs a test that exercises two concurrent writers —
the suite has nothing of the kind today, which is why the bug is invisible.

**3. H2 — name the CHS constants.**
The brief's headline item, and the highest readability return per line changed. Pure renaming, no
logic touched, fully guarded by the two existing geometry tests. Worth doing before H1 so the
extracted constants are available to whatever replaces the test's copy.

**4. H1 — make the qemu geometry test independent.**
Expose `chs_for_size` to the integration test, delete `chs_derived_size`, and replace it with a
table of spec-sourced `(size_bytes, C, H, S)` expectations. This is the item that converts a
passing-but-meaningless assertion into real coverage — and it should land before any further
refactoring of `chs_for_size`, since it is the only thing that would catch a mistake made during
H2.

**5. H7 and M4/M5 together — centralise the layout constants.**
One pass: `mod offsets` per header, `SECTOR_SIZE` promoted, `DiskType::to_u32`. Mechanical, wide,
and best done as a single change so the parser, the builder and all five fixture builders move at
once. Guarded by all 50 existing tests, which will fail loudly on any offset that shifts.

**6. M7 and M6 — collapse the three parallel `Option`s, then split `open_inner`.**
In that order: once `SparseState` exists, the five-tuple in `open_inner` collapses on its own and
the extraction becomes obvious rather than invented. Six defensive guards and one silent no-op
disappear.

**7. Everything else.**
M1/M2 (decoder and checksum duplication), M3 (bitmap helpers — note it also settles the bounds
asymmetry), M8 (`writable` naming), M9 (the `chs_for_size` doc), M10/M11 (the C-ABI wrapper and its
wrong panic names — worth pairing with the first C-ABI test the crate has ever had), M12
(`next_alloc_off`), M13 (the shared `TempPath`, which stops `synthetic.rs` leaking fixtures), then
the eight Low items as cleanup.

---

## Test results

No changes were made, so before and after are the same run.

| | Before | After |
|---|---|---|
| Tests passing | 50 | 50 (unchanged — no code modified) |
| Tests failing | 0 | 0 |
| `cargo clippy --all-targets -- -D warnings` | clean | clean |

Breakdown of the 50: `src/footer.rs` 7 · `src/dynamic.rs` 8 · `src/footer_build.rs` 5 ·
`tests/synthetic.rs` 25 · `tests/corruption.rs` 5. Plus 6 more in `tests/qemu_validation.rs`, which
are behind the `qemu-validation` feature and do not run by default.

Coverage gaps worth noting alongside the findings above:

- **`src/reader.rs` and `src/capi.rs` have no unit tests at all** (0 `#[test]` in either). `reader.rs`
  is the largest module in the crate at 755 lines and holds every High finding except H1/H2; it is
  covered only indirectly, through the integration suites. `capi.rs` — the crate's entire public C
  ABI, and the thing consumers actually link — is not exercised by any test in the repository.
- **No concurrency test anywhere**, which is why H4 and H5 sit undetected under a passing suite.
- **No test for a failing backing device**, so every rollback path in `allocate_block` is unexecuted.
- **Corruption testing covers the footer only** (`tests/corruption.rs` — cookie, checksum, disk
  type). The dynamic header, the BAT and the sector bitmaps have no corruption tests, which is the
  gap H6 falls through.
