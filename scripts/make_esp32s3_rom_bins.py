#!/usr/bin/env python3
"""Extract flat ROM (IROM) and DROM images from the genuine Espressif
ESP32-S3 boot-ROM ELF, for LabWired's faithful `--rom-boot` path.

The ESP32-S3 boot ROM is dual-mapped on silicon:
  * instruction bus 0x4000_0000..0x4006_0000  (384 KiB)  -> LABWIRED_ESP32S3_ROM
  * data bus        0x3FF0_0000..0x3FF2_0000  (128 KiB)  -> LABWIRED_ESP32S3_DROM

`configure_xtensa_esp32s3` loads each as a flat image based at its window
start, so this script walks the ELF program headers and lays every PT_LOAD
segment whose vaddr falls in a window at the window-relative offset.

The ROM blob is Espressif's copyright; it is NOT vendored. Point this at the
copy shipped with the ESP toolchain, e.g.
  ~/.platformio/tools/tool-esp-rom-elfs/esp32s3_rev0_rom.elf

Usage:
  make_esp32s3_rom_bins.py <esp32s3_rev0_rom.elf> [out_dir]
Writes <out_dir>/esp32s3_rom.bin and <out_dir>/esp32s3_drom.bin.
"""
import struct
import sys
from pathlib import Path

WINDOWS = {
    "esp32s3_rom.bin": (0x4000_0000, 0x6_0000),
    "esp32s3_drom.bin": (0x3FF0_0000, 0x2_0000),
}


def load_segments(elf: bytes):
    """Yield (p_vaddr, p_paddr, data) for each PT_LOAD with file content."""
    if elf[:4] != b"\x7fELF":
        raise SystemExit("not an ELF")
    e_phoff = struct.unpack_from("<I", elf, 0x1C)[0]
    e_phentsize = struct.unpack_from("<H", elf, 0x2A)[0]
    e_phnum = struct.unpack_from("<H", elf, 0x2C)[0]
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        p_type, p_offset, p_vaddr, p_paddr, p_filesz = struct.unpack_from(
            "<5I", elf, off
        )
        if p_type == 1 and p_filesz:  # PT_LOAD with file bytes
            yield p_vaddr, p_paddr, elf[p_offset : p_offset + p_filesz]


def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    elf = Path(sys.argv[1]).read_bytes()
    out = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("/tmp")
    out.mkdir(parents=True, exist_ok=True)
    segs = list(load_segments(elf))
    # The IROM window must be laid out by LOAD ADDRESS (p_paddr): code segments
    # have paddr==vaddr, but the ROM's `.data` is stored at an IROM LMA
    # (e.g. 0x400577A8, vaddr 0x3FCD7E00 in DRAM) and the ROM's own startup
    # data-init table copies from those LMAs into DRAM at boot. Keying on vaddr
    # would drop that `.data` source, so the copy writes zeros — e.g.
    # rom_cache_internal_table_ptr (0x3FCEFFC4) stays 0 and the cache routines
    # call a null vtable method. The DROM (data-bus) window is keyed by vaddr.
    KEY_BY_PADDR = {"esp32s3_rom.bin"}
    for name, (base, size) in WINDOWS.items():
        img = bytearray(size)
        placed = 0
        for vaddr, paddr, data in segs:
            addr = paddr if name in KEY_BY_PADDR else vaddr
            if base <= addr < base + size:
                rel = addr - base
                n = min(len(data), size - rel)
                img[rel : rel + n] = data[:n]
                placed += 1
        (out / name).write_bytes(img)
        print(f"{out / name}: {size} bytes, {placed} segments (base 0x{base:08x})")


if __name__ == "__main__":
    main()
