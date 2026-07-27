// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! Small STM32L476RG six-step BLDC controller used by the motor showcase.
//!
//! This deliberately uses registers directly so the article can show the
//! complete safety path. TIM1 drives three main/complementary pairs. Hall
//! feedback selects one of six commutation rows; a bounded proportional loop
//! adjusts PWM duty from Hall-transition period. A motor-fault input always
//! clears MOE and the external enable before reporting the fault.

#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use panic_halt as _;

const RCC: u32 = 0x4002_1000;
const RCC_AHB2ENR: *mut u32 = (RCC + 0x4c) as *mut u32;
const RCC_APB1ENR1: *mut u32 = (RCC + 0x58) as *mut u32;
const RCC_APB2ENR: *mut u32 = (RCC + 0x60) as *mut u32;
const GPIOA: u32 = 0x4800_0000;
const GPIOA_MODER: *mut u32 = GPIOA as *mut u32;
const GPIOA_AFRL: *mut u32 = (GPIOA + 0x20) as *mut u32;
const GPIOA_AFRH: *mut u32 = (GPIOA + 0x24) as *mut u32;
const GPIOB_MODER: *mut u32 = 0x4800_0400 as *mut u32;
const GPIOB_IDR: *const u32 = 0x4800_0410 as *const u32;
const GPIOB_ODR: *mut u32 = 0x4800_0414 as *mut u32;
const GPIOB_AFRH: *mut u32 = 0x4800_0424 as *mut u32;
const GPIOC_MODER: *mut u32 = 0x4800_0800 as *mut u32;
const GPIOC_IDR: *const u32 = 0x4800_0810 as *const u32;
const USART2: u32 = 0x4000_4400;
const USART2_CR1: *mut u32 = USART2 as *mut u32;
const USART2_BRR: *mut u32 = (USART2 + 0x0c) as *mut u32;
const USART2_ISR: *const u32 = (USART2 + 0x1c) as *const u32;
const USART2_TDR: *mut u32 = (USART2 + 0x28) as *mut u32;
const TIM1: u32 = 0x4001_2c00;
const TIM1_CR1: *mut u32 = TIM1 as *mut u32;
const TIM1_EGR: *mut u32 = (TIM1 + 0x14) as *mut u32;
const TIM1_CCMR1: *mut u32 = (TIM1 + 0x18) as *mut u32;
const TIM1_CCMR2: *mut u32 = (TIM1 + 0x1c) as *mut u32;
const TIM1_CCER: *mut u32 = (TIM1 + 0x20) as *mut u32;
const TIM1_PSC: *mut u32 = (TIM1 + 0x28) as *mut u32;
const TIM1_ARR: *mut u32 = (TIM1 + 0x2c) as *mut u32;
const TIM1_CCR1: *mut u32 = (TIM1 + 0x34) as *mut u32;
const TIM1_CCR2: *mut u32 = (TIM1 + 0x38) as *mut u32;
const TIM1_CCR3: *mut u32 = (TIM1 + 0x3c) as *mut u32;
const TIM1_BDTR: *mut u32 = (TIM1 + 0x44) as *mut u32;
const SYST_CSR: *mut u32 = 0xe000_e010 as *mut u32;
const SYST_RVR: *mut u32 = 0xe000_e014 as *mut u32;
const SYST_CVR: *mut u32 = 0xe000_e018 as *mut u32;

const MOE: u32 = 1 << 15;
const MOTOR_ENABLE: u32 = 1;
const PWM_PERIOD: u32 = 999;
const MIN_DUTY: u32 = 180;
const MAX_DUTY: u32 = 720;

// CCER rows: one sourcing main output and one sinking complementary output.
// Hall order is the conventional 001, 101, 100, 110, 010, 011 sequence.
const COMMUTATION: [u32; 8] = [
    0,                    // invalid 000
    (1 << 8) | (1 << 6),  // C+ B-
    (1 << 4) | (1 << 2),  // B+ A-
    (1 << 8) | (1 << 2),  // C+ A-
    (1 << 0) | (1 << 10), // A+ C-
    (1 << 0) | (1 << 6),  // A+ B-
    (1 << 4) | (1 << 10), // B+ C-
    0,                    // invalid 111
];
const STARTUP_HALL_ORDER: [u32; 6] = [1, 5, 4, 6, 2, 3];

#[inline(always)]
fn read(p: *const u32) -> u32 {
    unsafe { read_volatile(p) }
}

#[inline(always)]
fn write(p: *mut u32, value: u32) {
    unsafe { write_volatile(p, value) }
}

fn uart_puts(bytes: &[u8]) {
    for &byte in bytes {
        while read(USART2_ISR) & (1 << 7) == 0 {}
        write(USART2_TDR, byte as u32);
    }
}

fn shutdown() {
    // Safety invariant: hardware output gating precedes diagnostics.
    write(TIM1_BDTR, read(TIM1_BDTR) & !MOE);
    write(TIM1_CCER, 0);
    write(GPIOB_ODR, read(GPIOB_ODR) & !MOTOR_ENABLE);
}

fn fault(message: &[u8]) -> ! {
    shutdown();
    uart_puts(message);
    uart_puts(b"INVERTER OFF\r\n");
    loop {
        unsafe { core::arch::asm!("wfi") }
    }
}

fn commutate(hall: u32, duty: u32) {
    let low_edge = PWM_PERIOD - duty;
    let (a, b, c) = match hall {
        1 => (0, low_edge, duty), // C+ B-
        2 => (low_edge, duty, 0), // B+ A-
        3 => (low_edge, 0, duty), // C+ A-
        4 => (duty, 0, low_edge), // A+ C-
        5 => (duty, low_edge, 0), // A+ B-
        6 => (0, duty, low_edge), // B+ C-
        _ => (0, 0, 0),
    };
    write(TIM1_CCR1, a);
    write(TIM1_CCR2, b);
    write(TIM1_CCR3, c);
    write(TIM1_CCER, COMMUTATION[hall as usize]);
}

fn init() {
    write(RCC_AHB2ENR, read(RCC_AHB2ENR) | 0b111);
    write(RCC_APB1ENR1, read(RCC_APB1ENR1) | (1 << 17));
    write(RCC_APB2ENR, read(RCC_APB2ENR) | (1 << 11));

    // Real STM32 alternate-function routing:
    // PA2 USART2_TX AF7; PA8/9/10 TIM1 CH1/2/3 AF1.
    let a_clear = (0b11 << 4) | (0b11 << 16) | (0b11 << 18) | (0b11 << 20);
    write(
        GPIOA_MODER,
        (read(GPIOA_MODER) & !a_clear) | (0b10 << 4) | (0b10 << 16) | (0b10 << 18) | (0b10 << 20),
    );
    write(GPIOA_AFRL, (read(GPIOA_AFRL) & !(0xf << 8)) | (7 << 8));
    write(GPIOA_AFRH, (read(GPIOA_AFRH) & !0xfff) | 0x111);
    // PB13/14/15 TIM1 CH1N/2N/3N AF1, PB0 external enable, PB6/7 fault inputs.
    let b_clear =
        (0b11 << 0) | (0b11 << 12) | (0b11 << 14) | (0b11 << 26) | (0b11 << 28) | (0b11 << 30);
    write(
        GPIOB_MODER,
        (read(GPIOB_MODER) & !b_clear) | 1 | (0b10 << 26) | (0b10 << 28) | (0b10 << 30),
    );
    write(
        GPIOB_AFRH,
        (read(GPIOB_AFRH) & !(0xfff << 20)) | (0x111 << 20),
    );
    // PC0..2 Hall, PC3..5 encoder, PC6 motor fault, PC7 undervoltage.
    write(GPIOC_MODER, read(GPIOC_MODER) & !0xffff);
    write(GPIOB_ODR, MOTOR_ENABLE);
    write(USART2_BRR, 35);
    write(USART2_CR1, (1 << 0) | (1 << 3));
    write(SYST_RVR, 7_999); // 100 us control tick at 80 MHz.
    write(SYST_CVR, 0);
    write(SYST_CSR, (1 << 2) | 1); // core clock + enable, polled controller.

    // 80 MHz / (PSC+1) / (ARR+1) = 20 kHz.
    write(TIM1_PSC, 3);
    write(TIM1_ARR, PWM_PERIOD);
    write(TIM1_CCR1, 300);
    write(TIM1_CCR2, 300);
    write(TIM1_CCR3, 300);
    write(
        TIM1_CCMR1,
        (0b110 << 4) | (1 << 3) | (0b110 << 12) | (1 << 11),
    );
    write(TIM1_CCMR2, (0b110 << 4) | (1 << 3));
    write(TIM1_EGR, 1);
    write(TIM1_BDTR, MOE | 0x30); // MOE + readable dead time.
    write(TIM1_CR1, 1);
}

#[no_mangle]
pub extern "C" fn Reset() -> ! {
    init();
    uart_puts(b"BLDC READY\r\n");

    let mut previous_hall = read(GPIOC_IDR) & 7;
    let mut previous_encoder = (read(GPIOC_IDR) >> 3) & 3;
    let mut edge_period_ms = 0u32;
    let mut startup_ms = 0u32;
    let mut startup_step = 0usize;
    let mut valid_sequence_edges = 0u32;
    let mut in_band_edges = 0u32;
    let mut stall_ms = 0u32;
    let mut invalid_ms = 0u32;
    let mut motor_fault_ms = 0u32;
    let mut driver_fault_ms = 0u32;
    let mut overcurrent_ms = 0u32;
    let mut undervoltage_ms = 0u32;
    let mut encoder_edges = 0u32;
    let mut duty = 700u32;
    let mut hall_mode = false;

    loop {
        // The control and all debounce counters advance only on the
        // authoritative 100 us SysTick COUNTFLAG.
        if read(SYST_CSR) & (1 << 16) == 0 {
            continue;
        }
        let c = read(GPIOC_IDR);
        let b = read(GPIOB_IDR);
        motor_fault_ms = if c & (1 << 6) != 0 {
            motor_fault_ms + 1
        } else {
            0
        };
        undervoltage_ms = if c & (1 << 7) != 0 {
            undervoltage_ms + 1
        } else {
            0
        };
        overcurrent_ms = if b & (1 << 6) != 0 {
            overcurrent_ms + 1
        } else {
            0
        };
        driver_fault_ms = if b & (1 << 7) != 0 {
            driver_fault_ms + 1
        } else {
            0
        };
        if motor_fault_ms >= 20 {
            fault(b"FAULT STALL\r\n");
        }
        if overcurrent_ms >= 20 {
            fault(b"FAULT OVERCURRENT\r\n");
        }
        if undervoltage_ms >= 20 {
            fault(b"FAULT UNDERVOLTAGE\r\n");
        }
        if driver_fault_ms >= 20 {
            fault(b"FAULT DRIVER\r\n");
        }
        let hall = c & 7;
        let encoder = (c >> 3) & 3;
        encoder_edges += u32::from(encoder != previous_encoder);
        previous_encoder = encoder;
        edge_period_ms = edge_period_ms.saturating_add(1);
        startup_ms += 1;

        if hall == 0 || hall == 7 {
            invalid_ms += 1;
            if invalid_ms >= 100 {
                fault(b"FAULT HALL\r\n");
            }
            continue;
        }
        invalid_ms = 0;
        if hall != previous_hall {
            let previous_index = STARTUP_HALL_ORDER
                .iter()
                .position(|state| *state == previous_hall);
            let next_index = STARTUP_HALL_ORDER.iter().position(|state| *state == hall);
            if previous_index
                .zip(next_index)
                .is_some_and(|(old, new)| new == (old + 1) % 6)
            {
                valid_sequence_edges += 1;
            } else {
                valid_sequence_edges = 0;
            }
            previous_hall = hall;
            let period = edge_period_ms;
            edge_period_ms = 0;
            stall_ms = 0;
            if valid_sequence_edges >= 3 {
                hall_mode = true;
            }
            if hall_mode {
                // Target is a 0.4..1.2 ms Hall period. Slower (larger period)
                // raises duty; faster lowers it.
                let error = period.min(300) as i32 - 8;
                duty = (duty as i32 + error / 3).clamp(MIN_DUTY as i32, MAX_DUTY as i32) as u32;
                commutate(hall, duty);
                in_band_edges = if (4..=12).contains(&period) {
                    in_band_edges + 1
                } else {
                    0
                };
                if in_band_edges == 2 && encoder_edges > 0 {
                    uart_puts(b"TARGET REACHED\r\n");
                }
            }
        } else {
            stall_ms += 1;
            if hall_mode && stall_ms >= 500 {
                fault(b"FAULT STALL\r\n");
            }
        }
        if hall_mode {
            commutate(hall, duty);
        } else {
            // Open-loop startup continues only until three valid sequential
            // Hall edges have proved rotor motion.
            if startup_ms % 5 == 0 {
                startup_step += 1;
                let commanded = STARTUP_HALL_ORDER[startup_step % 6];
                commutate(commanded, duty);
            }
            if startup_ms >= 5_000 {
                fault(b"FAULT STARTUP\r\n");
            }
        }
    }
}
