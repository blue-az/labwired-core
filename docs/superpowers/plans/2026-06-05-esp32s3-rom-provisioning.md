# ESP32-S3 ROM Auto-Provisioning + Faithful Default Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the ESP32-S3 boot the real Espressif ROM **by default** — auto-discovering and extracting the ROM blob from the installed toolchain — with the thunk path kept only as a labelled no-blob fallback, and delete the dead `wifi_thunks` module.

**Architecture:** A new Rust module ports `scripts/make_esp32s3_rom_bins.py` (ELF → flat IROM/DROM images via `goblin`), discovers the toolchain ROM ELF, caches the extracted images keyed on the ELF content hash, and feeds them into `configure_xtensa_esp32s3`. The existing `LABWIRED_ESP32S3_ROM`/`_DROM` env vars still override; if nothing resolves, the model falls back to the thunk harness. A `boot_mode` field on `Esp32s3Wiring` records which path was taken.

**Tech Stack:** Rust, `goblin` (already a `labwired-core` dep), the existing `boot::` and `system::xtensa` modules.

**Spec:** `docs/superpowers/specs/2026-06-05-esp32s3-full-chip-model-design.md` (slices S1 + S2).

**Scope note:** This plan is slices S1+S2 only. Slice S3 (SVD coverage tool + faithful-mode gate) and the per-peripheral register/FSM-completeness slices (S4+) are separate plans.

---

## File Structure

- **Create** `crates/core/src/boot/esp32s3_rom.rs` — ROM provisioning: ELF extraction, toolchain discovery, caching. Declared as `pub mod esp32s3_rom;` in `crates/core/src/boot/mod.rs`.
- **Modify** `crates/core/src/boot/mod.rs` — add the module declaration.
- **Modify** `crates/core/src/system/xtensa.rs` — `Esp32s3Wiring` gains a `boot_mode` field; the ROM-load block (currently ~lines 905–930) calls `provision_rom_images()`.
- **Modify** `crates/core/src/peripherals/esp32s3/mod.rs` — remove `pub mod wifi_thunks;`.
- **Delete** `crates/core/src/peripherals/esp32s3/wifi_thunks.rs` — confirmed dead code.

---

## Task 1: ROM image extraction (Rust port of the Python script)

**Files:**
- Create: `crates/core/src/boot/esp32s3_rom.rs`
- Modify: `crates/core/src/boot/mod.rs`

- [ ] **Step 1: Declare the module**

In `crates/core/src/boot/mod.rs`, add after the existing `pub mod esp32s3;` line:

```rust
pub mod esp32s3_rom;
```

- [ ] **Step 2: Write the extraction module with a failing test**

Create `crates/core/src/boot/esp32s3_rom.rs`:

```rust
// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! ESP32-S3 boot-ROM provisioning for the faithful `--rom-boot` path.
//!
//! The ESP32-S3 boot ROM is Espressif copyright and is NOT vendored. Instead
//! we read the ROM ELF shipped with the user's installed toolchain and extract
//! the two flat images the model loads:
//!   * IROM (instruction bus) 0x4000_0000..0x4006_0000 (384 KiB)
//!   * DROM (data bus)        0x3FF0_0000..0x3FF2_0000 (128 KiB)
//!
//! This is a Rust port of `scripts/make_esp32s3_rom_bins.py`: PT_LOAD laid by
//! load-address (p_paddr) for IROM / vaddr for DROM, the boot ROM's `.data`
//! copy-source reconstruction, and a PROGBITS overlay for sections that live
//! in no PT_LOAD segment (e.g. `ets_rom_layout_p`).

use goblin::elf::program_header::PT_LOAD;
use goblin::elf::section_header::SHT_PROGBITS;
use goblin::elf::Elf;

pub const IROM_BASE: u32 = 0x4000_0000;
pub const IROM_SIZE: usize = 0x6_0000; // 384 KiB
pub const DROM_BASE: u32 = 0x3FF0_0000;
pub const DROM_SIZE: usize = 0x2_0000; // 128 KiB

const DRAM_LO: u32 = 0x3FC8_8000;
const DRAM_HI: u32 = 0x3FD0_0000;

/// Flat ROM images ready to load as `RamPeripheral`s at their window bases.
pub struct RomImages {
    pub irom: Vec<u8>,
    pub drom: Vec<u8>,
}

/// Extract the IROM and DROM flat images from the genuine ROM ELF bytes.
pub fn extract_rom_images(elf_bytes: &[u8]) -> Result<RomImages, String> {
    let elf = Elf::parse(elf_bytes).map_err(|e| format!("parse ROM ELF: {e}"))?;
    let irom = build_window(&elf, elf_bytes, IROM_BASE, IROM_SIZE, true);
    let drom = build_window(&elf, elf_bytes, DROM_BASE, DROM_SIZE, false);
    Ok(RomImages { irom, drom })
}

fn build_window(elf: &Elf, bytes: &[u8], base: u32, size: usize, by_paddr: bool) -> Vec<u8> {
    let mut img = vec![0u8; size];
    let win_end = base + size as u32;

    // 1. PT_LOAD pass — IROM keyed by load address (p_paddr), DROM by vaddr.
    for ph in &elf.program_headers {
        if ph.p_type != PT_LOAD || ph.p_filesz == 0 {
            continue;
        }
        let addr = if by_paddr { ph.p_paddr as u32 } else { ph.p_vaddr as u32 };
        if addr >= base && addr < win_end {
            let rel = (addr - base) as usize;
            let off = ph.p_offset as usize;
            let n = (ph.p_filesz as usize).min(size - rel);
            if off + n <= bytes.len() {
                img[rel..rel + n].copy_from_slice(&bytes[off..off + n]);
            }
        }
    }

    // 2. Reconstruct the boot ROM's `.data` copy sources (IROM window only).
    if by_paddr {
        populate_data_copy_sources(elf, bytes, &mut img, base);
    }

    // 3. Overlay PROGBITS sections that live in this window but in no PT_LOAD
    //    segment (e.g. the DROM `.rodata.interface` holding `ets_rom_layout_p`).
    //    Only fill bytes the PT_LOAD pass left as zero.
    for (sh_addr, data) in progbits_sections(elf, bytes) {
        if sh_addr >= base && sh_addr < win_end {
            let rel = (sh_addr - base) as usize;
            let n = data.len().min(size - rel);
            for i in 0..n {
                if img[rel + i] == 0 {
                    img[rel + i] = data[i];
                }
            }
        }
    }

    img
}

/// (sh_addr, bytes) for every SHT_PROGBITS section with an address + content,
/// sorted by address.
fn progbits_sections<'a>(elf: &Elf, bytes: &'a [u8]) -> Vec<(u32, &'a [u8])> {
    let mut v: Vec<(u32, &[u8])> = Vec::new();
    for sh in &elf.section_headers {
        if sh.sh_type == SHT_PROGBITS && sh.sh_size != 0 && sh.sh_addr != 0 {
            let off = sh.sh_offset as usize;
            let sz = sh.sh_size as usize;
            if off + sz <= bytes.len() {
                v.push((sh.sh_addr as u32, &bytes[off..off + sz]));
            }
        }
    }
    v.sort_by_key(|(a, _)| *a);
    v
}

/// Walk the in-image 16-byte copy-table quads (dst_start, dst_end, src, 0) and
/// fill each `src` LMA in the IROM image with the genuine bytes the matching
/// DRAM `dst` section holds, so the ROM's own startup copy lands real values.
fn populate_data_copy_sources(elf: &Elf, bytes: &[u8], irom: &mut [u8], irom_base: u32) {
    let sections = progbits_sections(elf, bytes);
    let vma_read = |addr: u32, n: usize| -> Vec<u8> {
        let mut out = vec![0u8; n];
        for (sa, data) in &sections {
            let sa = *sa;
            let end = sa + data.len() as u32;
            if sa <= addr + n as u32 && addr < end {
                let lo = addr.max(sa);
                let hi = (addr + n as u32).min(end);
                out[(lo - addr) as usize..(hi - addr) as usize]
                    .copy_from_slice(&data[(lo - sa) as usize..(hi - sa) as usize]);
            }
        }
        out
    };

    let irom_hi = irom_base + irom.len() as u32;
    let mut off = 0usize;
    while off + 16 <= irom.len() {
        let dst_s = u32::from_le_bytes(irom[off..off + 4].try_into().unwrap());
        let dst_e = u32::from_le_bytes(irom[off + 4..off + 8].try_into().unwrap());
        let src = u32::from_le_bytes(irom[off + 8..off + 12].try_into().unwrap());
        let term = u32::from_le_bytes(irom[off + 12..off + 16].try_into().unwrap());
        let ok = DRAM_LO <= dst_s
            && dst_s < DRAM_HI
            && dst_s <= dst_e
            && dst_e < DRAM_HI
            && irom_base <= src
            && src < irom_hi
            && term == 0
            && dst_e.wrapping_sub(dst_s) < 0x1_0000;
        if ok {
            let n = (dst_e - dst_s) as usize;
            if n != 0 {
                let vals = vma_read(dst_s, n);
                if vals.iter().any(|&b| b != 0) {
                    let rel = (src - irom_base) as usize;
                    let n2 = n.min(irom.len().saturating_sub(rel));
                    irom[rel..rel + n2].copy_from_slice(&vals[..n2]);
                }
            }
            off += 16;
        } else {
            off += 4;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal little-endian ELF32 with one PT_LOAD program header, used to
    /// verify the window-builder places file bytes at the right window offset.
    fn synthetic_elf_one_ptload(vaddr: u32, paddr: u32, payload: &[u8]) -> Vec<u8> {
        // Layout: [ehdr 52][phdr 32][payload]
        let e_phoff = 52u32;
        let e_phentsize = 32u16;
        let p_offset = (52 + 32) as u32;
        let mut elf = vec![0u8; p_offset as usize + payload.len()];
        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[4] = 1; // ELFCLASS32
        elf[5] = 1; // little-endian
        elf[6] = 1; // version
        // e_type=ET_EXEC(2), e_machine=94 (Xtensa), e_version=1
        elf[16..18].copy_from_slice(&2u16.to_le_bytes());
        elf[18..20].copy_from_slice(&94u16.to_le_bytes());
        elf[20..24].copy_from_slice(&1u32.to_le_bytes());
        elf[28..32].copy_from_slice(&e_phoff.to_le_bytes()); // e_phoff
        elf[42..44].copy_from_slice(&e_phentsize.to_le_bytes()); // e_phentsize
        elf[44..46].copy_from_slice(&1u16.to_le_bytes()); // e_phnum = 1
        // program header (ELF32): type, offset, vaddr, paddr, filesz, memsz, flags, align
        let ph = e_phoff as usize;
        elf[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
        elf[ph + 4..ph + 8].copy_from_slice(&p_offset.to_le_bytes());
        elf[ph + 8..ph + 12].copy_from_slice(&vaddr.to_le_bytes());
        elf[ph + 12..ph + 16].copy_from_slice(&paddr.to_le_bytes());
        elf[ph + 16..ph + 20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        elf[ph + 20..ph + 24].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        elf[ph + 24..ph + 28].copy_from_slice(&4u32.to_le_bytes());
        elf[ph + 28..ph + 32].copy_from_slice(&4u32.to_le_bytes());
        elf[p_offset as usize..].copy_from_slice(payload);
        elf
    }

    #[test]
    fn irom_window_keyed_by_paddr() {
        // A segment whose paddr is in the IROM window but vaddr is elsewhere
        // (mirrors the ROM's .data stored at an IROM LMA) must land in IROM.
        let payload = [0xAA, 0xBB, 0xCC, 0xDD];
        let elf = synthetic_elf_one_ptload(0x3FCD_7E00, IROM_BASE + 0x100, &payload);
        let images = extract_rom_images(&elf).expect("extract");
        assert_eq!(images.irom.len(), IROM_SIZE);
        assert_eq!(&images.irom[0x100..0x104], &payload);
    }

    #[test]
    fn drom_window_keyed_by_vaddr() {
        let payload = [0x11, 0x22, 0x33, 0x44];
        let elf = synthetic_elf_one_ptload(DROM_BASE + 0x200, 0xDEAD_0000, &payload);
        let images = extract_rom_images(&elf).expect("extract");
        assert_eq!(images.drom.len(), DROM_SIZE);
        assert_eq!(&images.drom[0x200..0x204], &payload);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail then pass**

Run: `cargo test -p labwired-core --lib esp32s3_rom -- --nocapture`
Expected: compiles, both tests PASS. (If they fail, the window keying is wrong — fix `build_window`.)

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/boot/mod.rs crates/core/src/boot/esp32s3_rom.rs
git commit -m "feat(esp32s3): Rust ROM-image extractor (port of make_esp32s3_rom_bins.py)"
```

---

## Task 2: Toolchain ROM-ELF discovery

**Files:**
- Modify: `crates/core/src/boot/esp32s3_rom.rs`

- [ ] **Step 1: Add a failing test for discovery preference order**

Add to the `tests` module in `crates/core/src/boot/esp32s3_rom.rs`:

```rust
    #[test]
    fn explicit_env_override_wins_for_discovery() {
        // A path set via LABWIRED_ESP32S3_ROM_ELF that exists is returned as-is.
        let tmp = std::env::temp_dir().join("labwired_test_rom_elf.bin");
        std::fs::write(&tmp, b"\x7fELF").unwrap();
        std::env::set_var("LABWIRED_ESP32S3_ROM_ELF", &tmp);
        let found = discover_rom_elf().expect("env override should resolve");
        assert_eq!(found, tmp);
        std::env::remove_var("LABWIRED_ESP32S3_ROM_ELF");
        let _ = std::fs::remove_file(&tmp);
    }
```

- [ ] **Step 2: Implement `discover_rom_elf`**

Add to `crates/core/src/boot/esp32s3_rom.rs` (above the `tests` module):

```rust
use std::path::PathBuf;

/// Locate the genuine ESP32-S3 ROM ELF in the user's installed toolchain.
///
/// Preference order:
///   1. `LABWIRED_ESP32S3_ROM_ELF` env var (explicit path).
///   2. PlatformIO: `~/.platformio/tools/tool-esp-rom-elfs/esp32s3_rev0_rom.elf`.
///   3. ESP-IDF: `~/.espressif/tools/esp-rom-elfs/<ver>/esp32s3_rev0_rom.elf`.
pub fn discover_rom_elf() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LABWIRED_ESP32S3_ROM_ELF") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let home = std::env::var("HOME").ok()?;

    let pio = PathBuf::from(format!(
        "{home}/.platformio/tools/tool-esp-rom-elfs/esp32s3_rev0_rom.elf"
    ));
    if pio.is_file() {
        return Some(pio);
    }

    // ESP-IDF nests the elfs under a version directory; scan one level.
    let idf_root = PathBuf::from(format!("{home}/.espressif/tools/esp-rom-elfs"));
    if let Ok(entries) = std::fs::read_dir(&idf_root) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("esp32s3_rev0_rom.elf");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p labwired-core --lib esp32s3_rom::tests::explicit_env_override_wins_for_discovery`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/boot/esp32s3_rom.rs
git commit -m "feat(esp32s3): discover toolchain ROM ELF (PlatformIO + ESP-IDF paths)"
```

---

## Task 3: Provision with content-hash caching

**Files:**
- Modify: `crates/core/src/boot/esp32s3_rom.rs`

- [ ] **Step 1: Add a failing test for the round-trip via a synthetic ELF**

Add to the `tests` module:

```rust
    #[test]
    fn provision_extracts_and_caches_from_elf_path() {
        let payload = [0x5A, 0x5B, 0x5C, 0x5D];
        let elf = synthetic_elf_one_ptload(IROM_BASE + 0x40, IROM_BASE + 0x40, &payload);
        let tmp = std::env::temp_dir().join("labwired_test_provision_rom.elf");
        std::fs::write(&tmp, &elf).unwrap();
        std::env::set_var("LABWIRED_ESP32S3_ROM_ELF", &tmp);
        // Ensure the env pre-extracted-bins path is not taken.
        std::env::remove_var("LABWIRED_ESP32S3_ROM");
        std::env::remove_var("LABWIRED_ESP32S3_DROM");

        let images = provision_rom_images().expect("provision");
        assert_eq!(images.irom.len(), IROM_SIZE);
        assert_eq!(&images.irom[0x40..0x44], &payload);

        std::env::remove_var("LABWIRED_ESP32S3_ROM_ELF");
        let _ = std::fs::remove_file(&tmp);
    }
```

- [ ] **Step 2: Implement `provision_rom_images` + helpers**

Add to `crates/core/src/boot/esp32s3_rom.rs`:

```rust
/// Resolve the ROM images for the faithful path, or `None` to fall back to
/// the thunk harness.
///
/// Order: explicit pre-extracted `LABWIRED_ESP32S3_ROM`/`_DROM` bins (back-compat)
/// → discover the toolchain ROM ELF, extract (cached by ELF content hash), load.
pub fn provision_rom_images() -> Option<RomImages> {
    // 1. Back-compat: explicit pre-extracted flat bins still win.
    if let (Ok(rp), Ok(dp)) = (
        std::env::var("LABWIRED_ESP32S3_ROM"),
        std::env::var("LABWIRED_ESP32S3_DROM"),
    ) {
        if let (Ok(irom), Ok(drom)) = (std::fs::read(&rp), std::fs::read(&dp)) {
            return Some(RomImages { irom, drom });
        }
    }

    // 2. Discover + extract (cached).
    let elf_path = discover_rom_elf()?;
    let elf_bytes = std::fs::read(&elf_path).ok()?;
    let key = fnv1a_64(&elf_bytes);
    let dir = cache_dir();
    let irom_path = dir.join(format!("esp32s3_irom_{key:016x}.bin"));
    let drom_path = dir.join(format!("esp32s3_drom_{key:016x}.bin"));

    if let (Ok(irom), Ok(drom)) = (std::fs::read(&irom_path), std::fs::read(&drom_path)) {
        if irom.len() == IROM_SIZE && drom.len() == DROM_SIZE {
            return Some(RomImages { irom, drom });
        }
    }

    let images = extract_rom_images(&elf_bytes).ok()?;
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&irom_path, &images.irom);
    let _ = std::fs::write(&drom_path, &images.drom);
    Some(images)
}

/// Cache directory for extracted ROM images (`$XDG_CACHE_HOME` or `~/.cache`).
fn cache_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(x).join("labwired/esp32s3-rom");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache/labwired/esp32s3-rom");
    }
    std::env::temp_dir().join("labwired-esp32s3-rom")
}

/// FNV-1a 64-bit hash (no extra deps; stable cache key across runs).
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p labwired-core --lib esp32s3_rom::tests::provision_extracts_and_caches_from_elf_path`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/boot/esp32s3_rom.rs
git commit -m "feat(esp32s3): provision_rom_images with content-hash cache"
```

---

## Task 4: Boot-mode field on `Esp32s3Wiring`

**Files:**
- Modify: `crates/core/src/system/xtensa.rs:63` (the `Esp32s3Wiring` struct)

- [ ] **Step 1: Add the enum + field**

In `crates/core/src/system/xtensa.rs`, immediately above `pub struct Esp32s3Wiring {`, add:

```rust
/// Which ROM path the ESP32-S3 model booted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Esp32s3BootMode {
    /// Real Espressif boot ROM loaded (faithful path, zero thunks).
    Faithful,
    /// No ROM blob found — running on the thunk harness (degraded).
    Harness,
}
```

Then add this field to the `Esp32s3Wiring` struct (after `dcache_backing`):

```rust
    pub boot_mode: Esp32s3BootMode,
```

- [ ] **Step 2: Make it compile (temporary value at the constructor)**

In `configure_xtensa_esp32s3`, find where `Esp32s3Wiring { cpu, icache_backing, dcache_backing }` is constructed (near the end of the function) and add `boot_mode: Esp32s3BootMode::Harness,` to the literal so the crate compiles. Task 5 sets the real value.

- [ ] **Step 3: Build**

Run: `cargo build -p labwired-core`
Expected: compiles (a `boot_mode` is now required in the struct literal).

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/system/xtensa.rs
git commit -m "feat(esp32s3): record boot mode (faithful vs harness) on wiring"
```

---

## Task 5: Wire provisioning into `configure_xtensa_esp32s3` (faithful default)

**Files:**
- Modify: `crates/core/src/system/xtensa.rs` (the ROM-load block, currently ~lines 905–949)

- [ ] **Step 1: Replace the env-only ROM load with provisioning**

In `crates/core/src/system/xtensa.rs`, replace the `let rom_loaded = match std::env::var("LABWIRED_ESP32S3_ROM") { … };` block (through the DROM load at ~line 949) with:

```rust
    // ── ROM: faithful real-silicon image (default) or thunk harness (fallback) ─
    // provision_rom_images() resolves the ROM either from explicit pre-extracted
    // bins (LABWIRED_ESP32S3_ROM/_DROM) or by discovering + extracting the ROM
    // ELF from the installed toolchain. None → no blob available → thunk harness.
    let boot_mode = match crate::boot::esp32s3_rom::provision_rom_images() {
        Some(images) => {
            let rom = RamPeripheral::with_image(0x6_0000, &images.irom);
            bus.add_peripheral("rom", 0x4000_0000, 0x6_0000, None, Box::new(rom));
            let drom = RamPeripheral::with_image(0x2_0000, &images.drom);
            bus.add_peripheral("drom", 0x3FF0_0000, 0x2_0000, None, Box::new(drom));
            eprintln!(
                "configure_xtensa_esp32s3: faithful ROM loaded ({} B IROM, {} B DROM) — real boot ROM, zero thunks",
                images.irom.len(),
                images.drom.len()
            );
            Esp32s3BootMode::Faithful
        }
        None => {
            let mut rom_bank = RomThunkBank::new(0x4000_0000, 0x6_0000);
            register_default_thunks(&mut rom_bank);
            bus.add_peripheral("rom_thunks", 0x4000_0000, 0x6_0000, None, Box::new(rom_bank));
            eprintln!(
                "configure_xtensa_esp32s3: ESP32-S3 ROM not found; running in degraded harness mode \
                 — install the ESP toolchain (or set LABWIRED_ESP32S3_ROM_ELF) for faithful simulation"
            );
            Esp32s3BootMode::Harness
        }
    };
```

> Note: this removes the separate `if rom_loaded { … DROM … }` block — the DROM is now loaded inside the `Some(images)` arm. **Verified during planning:** `rom_loaded` is referenced only at the original lines 907/925/936, all inside the block being replaced — there are no downstream uses, so nothing else needs updating.

- [ ] **Step 2: Set the real boot_mode in the returned wiring**

In the `Esp32s3Wiring { … }` constructor (end of the function), change `boot_mode: Esp32s3BootMode::Harness,` (added in Task 4) to:

```rust
        boot_mode,
```

- [ ] **Step 3: Build + run the existing tests**

Run: `cargo build -p labwired-core && cargo test -p labwired-core --lib esp32s3 2>&1 | tail -5`
Expected: compiles; esp32s3 unit tests PASS.

- [ ] **Step 4: Verify faithful boot end-to-end (manual smoke)**

Run (toolchain ROM ELF present, no env bins set):

```bash
LABWIRED_ESP32S3_FLASH=~/projects/SpiceDispenser/firmware/.pio/build/esp32-s3/firmware.factory.bin \
LABWIRED_ESP32S3_PCA9685=1 \
cargo run --release -p labwired-cli -- run \
  --chip configs/chips/esp32s3-zero.yaml \
  --firmware ~/projects/SpiceDispenser/firmware/.pio/build/esp32-s3/firmware.elf \
  --rom-boot --max-steps 40000000 2>&1 | grep -E 'faithful ROM loaded|PCA9685|INVALID_STATE'
```

Expected: `faithful ROM loaded …`, `PCA9685: channel 0 servo -> 180°`, and **no** `INVALID_STATE` — i.e. the SpiceDispenser now boots faithfully with **no env-var dance** (only the flash image is supplied).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/system/xtensa.rs
git commit -m "feat(esp32s3): faithful real-ROM path is now the default (auto-provisioned)"
```

---

## Task 6: ~~Remove the dead `wifi_thunks` module~~ — CANCELLED (not dead)

> **CANCELLED 2026-06-05.** During execution, Task 6 was found invalid: `wifi_thunks`
> is NOT dead code. `crates/core/tests/e2e_labwired_wifi.rs` (the ESP32-classic
> WiFi functional-model bring-up harness) uses ~25 symbols from it (SimNet +
> lwIP socket thunks). The original "dead code" assessment came from a grep that
> scanned only `src/` and missed `tests/`. The module is left untouched — it is a
> separate, out-of-scope WiFi workstream, not part of the S3 ROM path. The steps
> below are retained for the record but were not executed.

### Original (not executed):

**Files:**
- Modify: `crates/core/src/peripherals/esp32s3/mod.rs:41`
- Delete: `crates/core/src/peripherals/esp32s3/wifi_thunks.rs`

- [ ] **Step 1: Confirm it is dead**

Run: `grep -rn 'wifi_thunks\|WifiThunk' crates/ --include='*.rs' | grep -v 'esp32s3/mod.rs\|wifi_thunks.rs'`
Expected: **no output** (no references outside the module + its declaration).

- [ ] **Step 2: Remove the declaration and the file**

Delete the line `pub mod wifi_thunks;` from `crates/core/src/peripherals/esp32s3/mod.rs`, then:

```bash
git rm crates/core/src/peripherals/esp32s3/wifi_thunks.rs
```

- [ ] **Step 3: Build to confirm nothing referenced it**

Run: `cargo build -p labwired-core 2>&1 | tail -5`
Expected: compiles cleanly (no "unresolved import" / "cannot find" errors).

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/peripherals/esp32s3/mod.rs
git commit -m "chore(esp32s3): delete dead wifi_thunks module (radio out of scope)"
```

---

## Task 7: Full regression sweep

- [ ] **Step 1: Run the full workspace test suite**

Run: `cargo test --release --workspace 2>&1 | grep -E 'test result:|FAILED' | grep -v '0 failed'`
Expected: every suite `ok`; zero `FAILED`. (The `demo-blinky`/`nucleo-f407-i2c` e2e tests need their cross-compiled firmware built first — see the session notes; they are not affected by this change.)

- [ ] **Step 2: Confirm the harness fallback still works**

Run (force no-blob by pointing discovery at a non-existent ELF and clearing env bins):

```bash
LABWIRED_ESP32S3_ROM_ELF=/nonexistent.elf cargo run --release -p labwired-cli -- run \
  --chip configs/chips/esp32s3-zero.yaml \
  --firmware ~/projects/SpiceDispenser/firmware/.pio/build/esp32-s3/firmware.elf \
  --max-steps 100000 2>&1 | grep -E 'degraded harness mode'
```

Expected: prints the `degraded harness mode` notice (the fallback path still wires up).

- [ ] **Step 3: Commit any final touch-ups (if needed)**

```bash
git commit -am "test(esp32s3): regression sweep for faithful-default ROM provisioning" --allow-empty
```

---

## Self-Review notes (for the implementer)

- **Spec coverage:** S1 = Tasks 1–3, 5 (auto-extract + cache + discovery + load). S2 = Tasks 4–6 (faithful default switch, boot-mode telemetry, delete `wifi_thunks`). S3 (coverage tool + gate) and S4+ (peripherals) are **separate plans** — not in scope here.
- **Type consistency:** `RomImages { irom, drom }`, `provision_rom_images() -> Option<RomImages>`, `discover_rom_elf() -> Option<PathBuf>`, `Esp32s3BootMode::{Faithful,Harness}`, `Esp32s3Wiring.boot_mode` are used consistently across tasks.
- **Risk:** Task 5 deletes the old `rom_loaded` binding; Step 1's note flags checking downstream references. If `configure_xtensa_esp32s3` reads `rom_loaded` later (e.g. for a fast-boot branch), replace with `boot_mode == Esp32s3BootMode::Faithful`.
