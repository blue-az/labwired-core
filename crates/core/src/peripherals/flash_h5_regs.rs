// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.

//! STM32H5 FLASH register offsets + bitfields (RM0481 §7).
//!
//! Register offsets and reset values cross-checked against NUCLEO-H563ZI
//! silicon via SWD (2026-06-20): a read of 0x4002_2000×24 confirmed
//! ACR=0x13, OPTCR=0x1, NSSR=0x0, NSCR=0x1, OPTSR_CUR=OPTSR_PRG=0x2D30_EDF8
//! (bit31/SWAP_BANK clear → bank 1 active), matching the model reset values.
//! Bitfield positions (SER/STRT/SNB/SWAP_BANK/OBL_LAUNCH) are architectural
//! per RM0481 §7 (not reset-readable).

#![allow(dead_code)]

// ── Register offsets (relative to FLASH base 0x4002_2000) ───────────────────

pub const NSSR_OFF: u64 = 0x20;
pub const NSCR_OFF: u64 = 0x28;
pub const OPTCR_OFF: u64 = 0x1C;
pub const OPTSR_CUR_OFF: u64 = 0x50;
pub const OPTSR_PRG_OFF: u64 = 0x54;

// ── FLASH_NSCR bitfields (RM0481 §7.9.9) ────────────────────────────────────

/// Bit 0 — LOCK: FLASH_NSCR lock bit. 1 = locked (reset state).
pub const NSCR_LOCK: u32 = 1 << 0;
/// Bit 1 — PG: non-secure programming enable.
pub const NSCR_PG: u32 = 1 << 1;
/// Bit 2 — SER: non-secure sector erase.
pub const NSCR_SER: u32 = 1 << 2;
/// Bit 3 — BER: non-secure bank erase.
pub const NSCR_BER: u32 = 1 << 3;
/// Bit 5 — STRT: non-secure start erase/program.
pub const NSCR_STRT: u32 = 1 << 5;
/// Bits [12:6] — SNB: sector number (7 bits).
pub const NSCR_SNB_SHIFT: u32 = 6;
pub const NSCR_SNB_MASK: u32 = 0x7F << NSCR_SNB_SHIFT;
/// Bit 31 — BKSEL: bank select (0 = bank 1, 1 = bank 2).
pub const NSCR_BKSEL: u32 = 1 << 31;

// ── FLASH_NSSR bitfields (RM0481 §7.9.8) ────────────────────────────────────

/// Bit 0 — BSY: non-secure busy.
pub const NSSR_BSY: u32 = 1 << 0;

// ── FLASH_OPTSR_CUR / OPTSR_PRG bitfields (RM0481 §7.9.14) ─────────────────

/// Bit 31 — SWAP_BANK: bank-swap option bit (0 = no swap, 1 = swapped).
/// OPTSR_CUR bit 31 = 0 on the captured NUCLEO-H563ZI (0x2D30_EDF8).
pub const OPTSR_SWAP_BANK: u32 = 1 << 31;

// ── FLASH_OPTCR bitfields (RM0481 §7.9.13) ──────────────────────────────────

/// Bit 27 — OBL_LAUNCH: option byte loading launch.
pub const OPTCR_OBL_LAUNCH: u32 = 1 << 27;

// ── Address / geometry constants ─────────────────────────────────────────────

/// Flash base address (both banks start here before any bank swap).
pub const FLASH_BASE: u64 = 0x0800_0000;
/// Per-bank size: 1 MB (STM32H563ZI has 2 × 1 MB).
pub const BANK_SIZE: u64 = 0x10_0000;
/// Sector size: 8 KB (RM0481 §7.1).
pub const SECTOR_SIZE: u64 = 0x2000;
