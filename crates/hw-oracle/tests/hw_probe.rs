// Diagnostic probe: spawn OpenOCD, reset_halt, dump key registers + memory.
// Used to debug the hw-oracle ELF execute regression.

#![cfg(feature = "hw-oracle")]

use labwired_hw_oracle::flash::TargetBoard;
use labwired_hw_oracle::openocd::OpenOcd;

#[test]
#[ignore = "hw-probe: replicate capture_hw_state step by step"]
fn hw_probe_fibonacci_full_capture_replica() {
    use goblin::elf::program_header::PT_LOAD;
    use goblin::elf::Elf;

    let board = TargetBoard::detect().expect("board detect");
    let mut oc = OpenOcd::spawn_for(&board).expect("openocd spawn");

    eprintln!("=== Step 1: reset_halt + halt ===");
    oc.reset_halt().expect("reset_halt");
    oc.halt().expect("halt after reset_halt");
    eprintln!("pc = 0x{:08x}", oc.read_register("pc").unwrap_or(0xDEAD));

    eprintln!("=== Step 2: fill_memory(IRAM, 0, 1028 words) ===");
    let prog_zero_words: usize = (0x1000 + 0x40) / 4; // = 1040
    oc.fill_memory(0x40370000, 0, prog_zero_words)
        .expect("fill IRAM");
    eprintln!("pc = 0x{:08x}", oc.read_register("pc").unwrap_or(0xDEAD));

    eprintln!("=== Step 3: fill_memory(DRAM+0x1000, 0, 16 words) ===");
    oc.fill_memory(0x3FC88000 + 0x1000, 0, 16)
        .expect("fill DRAM");
    eprintln!("pc = 0x{:08x}", oc.read_register("pc").unwrap_or(0xDEAD));

    eprintln!("=== Step 4: Load ELF segments ===");
    let elf_bytes = std::fs::read("../../fixtures/xtensa-asm/fibonacci.elf").expect("read ELF");
    let elf = Elf::parse(&elf_bytes).expect("parse ELF");
    for ph in &elf.program_headers {
        if ph.p_type != PT_LOAD || ph.p_filesz == 0 {
            continue;
        }
        let vaddr = ph.p_vaddr as u32;
        let size = ph.p_filesz as usize;
        let file_off = ph.p_offset as usize;
        let seg_data = &elf_bytes[file_off..file_off + size];
        let mut padded = seg_data.to_vec();
        while padded.len() % 4 != 0 {
            padded.push(0);
        }
        let words: Vec<u32> = padded
            .chunks(4)
            .map(|c| {
                let mut w = [0u8; 4];
                w[..c.len()].copy_from_slice(c);
                u32::from_le_bytes(w)
            })
            .collect();
        oc.write_memory(vaddr, &words).expect("write seg");
    }
    let entry_pc = elf.entry as u32;
    let readback = oc.read_memory(entry_pc, 9).expect("readback");
    eprintln!(
        "First word @ entry: 0x{:08x} (expected 0x320aa022)",
        readback[0]
    );

    eprintln!("=== Step 5: zero windowbase/windowstart ===");
    oc.write_register("windowbase", 0).expect("wb");
    oc.write_register("windowstart", 1).expect("ws");

    eprintln!("=== Step 6: zero a0..a15 ===");
    for i in 0u32..16 {
        oc.write_register(&format!("a{}", i), 0).expect("zero ar");
    }

    eprintln!("=== Step 7: zero sar/scompare1 ===");
    let _ = oc.write_register("sar", 0);
    let _ = oc.write_register("scompare1", 0);

    eprintln!("=== Step 8: clean PS ===");
    oc.write_register("ps", 1u32 << 18).expect("clean ps");

    eprintln!("=== Step 9: set PC ===");
    oc.write_register("pc", entry_pc).expect("set pc");

    eprintln!("\n=== Pre-resume ===");
    eprintln!("pc = 0x{:08x}", oc.read_register("pc").unwrap_or(0xDEAD));
    eprintln!("a2 = 0x{:08x}", oc.read_register("a2").unwrap_or(0xDEAD));
    eprintln!("ps = 0x{:08x}", oc.read_register("ps").unwrap_or(0xDEAD));

    eprintln!("=== Step 10: resume + wait ===");
    oc.resume().expect("resume");
    oc.wait_until_halted(std::time::Duration::from_secs(5))
        .expect("wait");

    eprintln!("\n=== After halt ===");
    eprintln!("pc = 0x{:08x}", oc.read_register("pc").unwrap_or(0xDEAD));
    eprintln!(
        "a2 = 0x{:08x} (expected 0x37)",
        oc.read_register("a2").unwrap_or(0xDEAD)
    );
    eprintln!("a3 = 0x{:08x}", oc.read_register("a3").unwrap_or(0xDEAD));
}

#[test]
#[ignore = "hw-probe: capture actual WB on real silicon at OF4 vector entry"]
fn hw_probe_overflow_captures_wb_on_silicon() {
    // The H7.2 test sees PC = vecbase + 0x3C0 (DoubleExceptionVector) on
    // silicon because BREAK with PS.EXCM=1 fires double-exception. To
    // capture the actual WindowBase at OF4 entry, place a non-BREAK
    // sequence in the OF4 vector that:
    //   1. RSR a4, WINDOWBASE       — reads WB into a4
    //   2. WSR a4, EXCSAVE1          — stashes WB in EXCSAVE1 (always readable)
    //   3. RSR a5, PS                — reads PS into a5
    //   4. WSR a5, EXCSAVE2          — stashes PS in EXCSAVE2
    //   5. WSR.PS to clear EXCM       — so BREAK works cleanly
    //   6. BREAK 1, 15                — clean halt (now that EXCM=0)
    //
    // After halt, read EXCSAVE1 / EXCSAVE2 to see what WB and PS were at
    // OF4 entry.

    let board = TargetBoard::detect().expect("board detect");
    let mut oc = OpenOcd::spawn_for(&board).expect("openocd spawn");
    oc.reset_halt().expect("reset_halt");
    oc.halt().expect("halt");

    const IRAM_BASE: u32 = 0x40370000;
    const VECBASE: u32 = IRAM_BASE + 0x800;

    // Zero IRAM region we use.
    oc.fill_memory(IRAM_BASE, 0, 0x1000 / 4 + 64)
        .expect("fill IRAM");

    // ── Main program at IRAM_BASE: CALL4 → ENTRY at +8 ─────────────────
    // CALL4 imm18=1, target = IRAM_BASE+8, encoding 0x000055 LE bytes 55 00 00.
    // ENTRY a1, 32: encoding 0x004136, LE bytes 36 41 00.
    oc.write_memory(IRAM_BASE, &[0x00000055]).expect("CALL4");
    oc.write_memory(IRAM_BASE + 4, &[0x00004136])
        .expect("ENTRY part");

    // ── OF4 vector at VECBASE+0x000: capture-WB sequence ─────────────────
    //
    // Encodings (all 3 bytes, LE):
    //   rsr.windowbase a4 = 0x034840 → bytes 40 48 03
    //   wsr.excsave1   a4 = 0x13D040 → bytes 40 D0 13
    //   rsr.ps         a5 = 0x03E650 → bytes 50 E6 03
    //   wsr.excsave2   a5 = 0x13D150 → bytes 50 D1 13
    //   ... actually let me use simpler approach: just BREAK with EXCM=1 expected
    //   and read PS.OWB / WindowBase via OpenOCD AFTER the double-exception halt.
    //
    // The DoubleException vector at VECBASE+0x3C0 will be reached. Place a
    // BREAK there too — even though EXCM is now stuck at 1 (from the double
    // exception), at least the CPU stops there and we can read state.

    // Place infinite loop `j .` at OF4 vector so we can capture state via
    // OpenOCD force-halt without triggering DoubleException.
    //
    // Encoding for `j .` (jump to self): J relative target = current PC.
    // The J instruction is op0=6, with 18-bit signed PC-relative offset.
    // Per Xtensa: J target is computed as `PC + 4 + sign_extend(offset)`.
    // For target = PC, offset = -4. Encoding: 0x000006 | (((-4) & 0x3FFFF) << 6)
    //   offset = -4 → 0x3FFFC (18-bit two's complement)
    //   shifted by 6: 0x3FFFC << 6 = 0xFFFF00
    //   OR with op0=6: 0xFFFF06
    //   LE bytes: 06 FF FF
    oc.write_memory(VECBASE, &[0x00FFFF06]).expect("J . in OF4");

    // Set up state for overflow trigger.
    oc.write_register("windowbase", 0).expect("wb");
    oc.write_register("windowstart", 0x0005).expect("ws"); // bits 0, 2
    for i in 0u32..16 {
        let _ = oc.write_register(&format!("a{}", i), 0);
    }
    oc.write_register("ps", 1u32 << 18).expect("clean ps"); // WOE=1, EXCM=0
    let _ = oc.write_register("vecbase", VECBASE);
    oc.write_register("pc", IRAM_BASE).expect("pc");

    // Try invalidating I-cache via OpenOCD. ESP32-S3's icache_invalidate is
    // a function in ROM; use the TCL `mww` to write to the cache control reg
    // or just spam the EXTW instruction. Actually OpenOCD's `flush_count`
    // is for d-cache. Let's try the `xtensa_icache_invalidate` if available.
    let _ = oc.tcl("xtensa flush_count");
    let _ = oc.tcl("xtensa flush_cache");

    eprintln!("=== Pre-resume ===");
    eprintln!(
        "pc       = 0x{:08x}",
        oc.read_register("pc").unwrap_or(0xDEAD)
    );
    eprintln!(
        "ps       = 0x{:08x}",
        oc.read_register("ps").unwrap_or(0xDEAD)
    );
    eprintln!(
        "wb       = {}",
        oc.read_register("windowbase").unwrap_or(0xDEAD)
    );
    eprintln!(
        "ws       = 0x{:04x}",
        oc.read_register("windowstart").unwrap_or(0xDEAD)
    );

    oc.resume().expect("resume");
    // Let it spin for ~50ms in the J-loop, then force-halt via OpenOCD.
    std::thread::sleep(std::time::Duration::from_millis(100));
    oc.halt().expect("force halt");

    eprintln!("\n=== After halt ===");
    eprintln!(
        "pc       = 0x{:08x}",
        oc.read_register("pc").unwrap_or(0xDEAD)
    );
    eprintln!(
        "ps       = 0x{:08x}",
        oc.read_register("ps").unwrap_or(0xDEAD)
    );
    eprintln!(
        "wb       = {}",
        oc.read_register("windowbase").unwrap_or(0xDEAD)
    );
    eprintln!(
        "ws       = 0x{:04x}",
        oc.read_register("windowstart").unwrap_or(0xDEAD)
    );
    eprintln!(
        "epc1     = 0x{:08x}",
        oc.read_register("epc1").unwrap_or(0xDEAD)
    );
    let ps = oc.read_register("ps").unwrap_or(0);
    eprintln!("ps.owb   = {} (bits 11:8)", (ps >> 8) & 0xF);
    eprintln!("ps.callinc = {} (bits 17:16)", (ps >> 16) & 0x3);
    eprintln!("ps.excm  = {}", (ps >> 4) & 1);
    eprintln!("ps.woe   = {}", (ps >> 18) & 1);
    eprintln!("ps.intlevel = {}", ps & 0xF);
    // Read AR registers to see where each frame's "a1" got written
    eprintln!("\nAR file:");
    for i in 0u32..16 {
        let v = oc.read_register(&format!("a{}", i)).unwrap_or(0xDEAD);
        eprintln!("  a{:<2} = 0x{:08x}", i, v);
    }
}

#[test]
#[ignore = "hw-probe: load fibonacci.elf manually, run it, check a2"]
fn hw_probe_fibonacci_manual() {
    use goblin::elf::program_header::PT_LOAD;
    use goblin::elf::Elf;

    let board = TargetBoard::detect().expect("board detect");
    let mut oc = OpenOcd::spawn_for(&board).expect("openocd spawn");
    oc.reset_halt().expect("reset_halt");
    oc.halt().expect("halt");

    eprintln!("=== Loading fibonacci.elf ===");
    let elf_bytes = std::fs::read("../../fixtures/xtensa-asm/fibonacci.elf").expect("read ELF");
    let elf = Elf::parse(&elf_bytes).expect("parse ELF");
    eprintln!("ELF entry = 0x{:08x}", elf.entry);

    for ph in &elf.program_headers {
        if ph.p_type != PT_LOAD || ph.p_filesz == 0 {
            continue;
        }
        let vaddr = ph.p_vaddr as u32;
        let size = ph.p_filesz as usize;
        let file_off = ph.p_offset as usize;
        eprintln!(
            "LOAD vaddr=0x{:08x} size=0x{:x} file_off=0x{:x}",
            vaddr, size, file_off
        );
        let seg_data = &elf_bytes[file_off..file_off + size];
        let mut padded = seg_data.to_vec();
        while padded.len() % 4 != 0 {
            padded.push(0);
        }
        let words: Vec<u32> = padded
            .chunks(4)
            .map(|c| {
                let mut w = [0u8; 4];
                w[..c.len()].copy_from_slice(c);
                u32::from_le_bytes(w)
            })
            .collect();
        oc.write_memory(vaddr, &words).expect("write_memory");
    }

    let entry_pc = elf.entry as u32;

    // Read back what we wrote
    let readback = oc.read_memory(entry_pc, 9).expect("read program");
    eprintln!("Loaded program @ 0x{:08x}:", entry_pc);
    for (i, w) in readback.iter().enumerate() {
        eprintln!("  +0x{:02x}: 0x{:08x}", i * 4, w);
    }

    // Set up clean state and run
    oc.write_register("windowbase", 0).expect("wb");
    oc.write_register("windowstart", 1).expect("ws");
    for i in 0u32..16 {
        oc.write_register(&format!("a{}", i), 0).expect("zero reg");
    }
    oc.write_register("ps", 1u32 << 18).expect("clean ps");
    oc.write_register("pc", entry_pc).expect("set pc");

    eprintln!("\n=== Pre-resume ===");
    eprintln!("pc = 0x{:08x}", oc.read_register("pc").unwrap_or(0xDEAD));
    eprintln!("a2 = 0x{:08x}", oc.read_register("a2").unwrap_or(0xDEAD));
    eprintln!("a3 = 0x{:08x}", oc.read_register("a3").unwrap_or(0xDEAD));
    eprintln!("ps = 0x{:08x}", oc.read_register("ps").unwrap_or(0xDEAD));

    oc.resume().expect("resume");
    oc.wait_until_halted(std::time::Duration::from_secs(5))
        .expect("wait");

    eprintln!("\n=== After halt ===");
    eprintln!("pc = 0x{:08x}", oc.read_register("pc").unwrap_or(0xDEAD));
    eprintln!("a0 = 0x{:08x}", oc.read_register("a0").unwrap_or(0xDEAD));
    eprintln!("a1 = 0x{:08x}", oc.read_register("a1").unwrap_or(0xDEAD));
    eprintln!(
        "a2 = 0x{:08x} (expected 0x37 = 55)",
        oc.read_register("a2").unwrap_or(0xDEAD)
    );
    eprintln!("a3 = 0x{:08x}", oc.read_register("a3").unwrap_or(0xDEAD));
    eprintln!("a4 = 0x{:08x}", oc.read_register("a4").unwrap_or(0xDEAD));
    eprintln!("a5 = 0x{:08x}", oc.read_register("a5").unwrap_or(0xDEAD));
}

#[test]
#[ignore = "hw-probe: spawns OpenOCD and prints state — for manual debugging"]
fn hw_probe_post_reset_state() {
    let board = TargetBoard::detect().expect("board detect");
    let mut oc = OpenOcd::spawn_for(&board).expect("openocd spawn");

    eprintln!("=== After spawn (no reset yet) ===");
    eprintln!(
        "pc       = 0x{:08x}",
        oc.read_register("pc").unwrap_or(0xDEAD)
    );
    eprintln!(
        "a0       = 0x{:08x}",
        oc.read_register("a0").unwrap_or(0xDEAD)
    );
    eprintln!(
        "a2       = 0x{:08x}",
        oc.read_register("a2").unwrap_or(0xDEAD)
    );

    eprintln!("\n=== reset_halt ===");
    oc.reset_halt().expect("reset_halt");

    eprintln!(
        "pc       = 0x{:08x}",
        oc.read_register("pc").unwrap_or(0xDEAD)
    );
    eprintln!(
        "a0       = 0x{:08x}",
        oc.read_register("a0").unwrap_or(0xDEAD)
    );
    eprintln!(
        "a2       = 0x{:08x}",
        oc.read_register("a2").unwrap_or(0xDEAD)
    );
    eprintln!(
        "ps       = 0x{:08x}",
        oc.read_register("ps").unwrap_or(0xDEAD)
    );
    eprintln!(
        "wb       = 0x{:08x}",
        oc.read_register("windowbase").unwrap_or(0xDEAD)
    );
    eprintln!(
        "ws       = 0x{:08x}",
        oc.read_register("windowstart").unwrap_or(0xDEAD)
    );

    eprintln!("\n=== explicit halt ===");
    oc.halt().expect("halt");
    eprintln!(
        "pc       = 0x{:08x}",
        oc.read_register("pc").unwrap_or(0xDEAD)
    );

    eprintln!("\n=== Write a2=0x12345678, read back ===");
    oc.write_register("a2", 0x12345678).expect("write a2");
    let v = oc.read_register("a2").unwrap_or(0xDEAD);
    eprintln!("a2 after write = 0x{:08x} (expected 0x12345678)", v);

    eprintln!("\n=== Write IRAM[0x40370000..0x4037000F], read back ===");
    oc.write_memory(
        0x40370000,
        &[0xAABBCCDD, 0x11223344, 0x55667788, 0xCAFEBABE],
    )
    .expect("write_memory");
    let words = oc.read_memory(0x40370000, 4).expect("read_memory");
    for (i, w) in words.iter().enumerate() {
        eprintln!("IRAM[0x{:08x}] = 0x{:08x}", 0x40370000 + i * 4, w);
    }

    eprintln!("\n=== Set PC = 0x40370000, simple program: movi a2, 42 ; break 1,15 ===");
    // movi a2, 42 = 0x00A2C2 (op0=2, t=2, s=0xC, r=2 ... actually let me encode properly)
    // movi at, imm: format is op0=A, t=at, s=imm[7:4], r=imm[3:0]
    // movi a2, 42 (0x2A): op0=2(LSCI no wait MOVI is in BRI)
    // Actually MOVI ar,imm12 is BRI8 format, op0=2, n=1, m=2, ar=at, imm12 = imm
    // bytes: imm-low(8 bits), op0|n|m|imm-hi(4)... let me just use the standard encoding from xtensa-esp32s3-elf-as
    // movi a2, 42 → 0x2a a2 02 (LE) = op0=2, t=2, s=A(=10 = imm[3:0] of imm bits), r=2(=imm[7:4])
    // Actually it's simpler: just hardcoded movi and break sequence.

    // Bytes for `movi a2, 42; break 1, 15` (verified via xtensa-esp32s3-elf-as):
    //   movi a2, 42  = 22 A0 2A   (encoding 0x2aa022, LE bytes 22 A0 2A)
    //   break 1, 15  = F0 41 00
    let prog = [
        0x22, 0xA0, 0x2A, // movi a2, 42
        0xF0, 0x41, 0x00, // break 1, 15
        0x00, 0x00, // padding to 4 bytes
        0x00, 0x00, 0x00, 0x00,
    ];
    let words: Vec<u32> = prog
        .chunks(4)
        .map(|c| {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            u32::from_le_bytes(w)
        })
        .collect();
    oc.write_memory(0x40370000, &words).expect("write program");

    // Read back to confirm
    let readback = oc.read_memory(0x40370000, 2).expect("read program");
    eprintln!(
        "Program in IRAM: {:08x?} (expected first word 0x022A_2A02)",
        readback
    );

    // Set PC, set PS to runnable, set a2=0 then resume
    oc.write_register("a2", 0).expect("clear a2");
    oc.write_register("ps", 1u32 << 18).expect("clean ps"); // WOE=1, EXCM=0, INTLEVEL=0
    oc.write_register("pc", 0x40370000).expect("set pc");

    eprintln!("\n=== Pre-resume ===");
    eprintln!(
        "pc       = 0x{:08x}",
        oc.read_register("pc").unwrap_or(0xDEAD)
    );
    eprintln!(
        "a2       = 0x{:08x}",
        oc.read_register("a2").unwrap_or(0xDEAD)
    );
    eprintln!(
        "ps       = 0x{:08x}",
        oc.read_register("ps").unwrap_or(0xDEAD)
    );

    eprintln!("\n=== Resume + wait ===");
    oc.resume().expect("resume");
    oc.wait_until_halted(std::time::Duration::from_secs(3))
        .expect("wait");

    eprintln!("\n=== After halt ===");
    eprintln!(
        "pc       = 0x{:08x}",
        oc.read_register("pc").unwrap_or(0xDEAD)
    );
    eprintln!(
        "a2       = 0x{:08x} (expected 42 = 0x2A)",
        oc.read_register("a2").unwrap_or(0xDEAD)
    );
}
