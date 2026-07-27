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
const GPIOA_ODR: *mut u32 = (GPIOA + 0x14) as *mut u32;
const GPIOA_IDR: *const u32 = (GPIOA + 0x10) as *const u32;
const GPIOB_IDR: *const u32 = 0x4800_0410 as *const u32;
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
const MOTOR_FAULT: u32 = 1 << 7;
const HALL_MASK: u32 = 0b111 << 1;
const PWM_PERIOD: u32 = 999;
const MIN_DUTY: u32 = 180;
const MAX_DUTY: u32 = 720;

// CCER rows: one sourcing main output and one sinking complementary output.
// Hall order is the conventional 001, 101, 100, 110, 010, 011 sequence.
const COMMUTATION: [u32; 8] = [
    0,                    // invalid 000
    (1 << 0) | (1 << 6),  // A+ B-
    (1 << 4) | (1 << 10), // B+ C-
    (1 << 4) | (1 << 2),  // B+ A-
    (1 << 8) | (1 << 2),  // C+ A-
    (1 << 0) | (1 << 10), // A+ C-
    (1 << 8) | (1 << 6),  // C+ B-
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
    write(GPIOA_ODR, read(GPIOA_ODR) & !MOTOR_ENABLE);
}

fn fault(message: &[u8]) -> ! {
    shutdown();
    uart_puts(message);
    uart_puts(b"INVERTER OFF\r\n");
    loop {
        unsafe { core::arch::asm!("wfi") }
    }
}

fn init() {
    write(RCC_AHB2ENR, read(RCC_AHB2ENR) | 0b11);
    write(RCC_APB1ENR1, read(RCC_APB1ENR1) | (1 << 17));
    write(RCC_APB2ENR, read(RCC_APB2ENR) | (1 << 11));

    // PA0 external enable output; PA1..PA7 Hall/encoder/fault inputs.
    write(GPIOA_MODER, (read(GPIOA_MODER) & !0xffff) | 1);
    write(GPIOA_ODR, MOTOR_ENABLE);
    write(USART2_BRR, 35);
    write(USART2_CR1, (1 << 0) | (1 << 3));
    write(SYST_RVR, 79_999); // 1 ms heartbeat at 80 MHz.
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

    let mut previous_hall = 0u32;
    let mut since_edge = 0u32;
    let mut invalid_hall = 0u32;
    let mut stationary = 0u32;
    let mut edges = 0u32;
    let mut duty = 300u32;
    let mut startup_ticks = 0u32;
    let mut startup_step = 0usize;
    let mut target_reported = false;

    loop {
        let inputs = read(GPIOA_IDR);
        if inputs & MOTOR_FAULT != 0 {
            fault(b"FAULT STALL\r\n");
        }
        if read(GPIOB_IDR) & (1 << 7) != 0 {
            fault(b"FAULT CURRENT\r\n");
        }
        let hall = (inputs & HALL_MASK) >> 1;
        // Open-loop alignment and ramp: advance the canonical electrical
        // sequence at a fixed, bounded cadence. Hall control takes over after
        // eight electrical revolutions; this avoids requiring rotor motion to
        // discover the first sector.
        if startup_step < 48 {
            startup_ticks += 1;
            if startup_ticks >= 100 {
                startup_ticks = 0;
                startup_step += 1;
                let commanded = STARTUP_HALL_ORDER[startup_step % 6];
                write(TIM1_CCER, COMMUTATION[commanded as usize]);
            }
            if startup_step == 48 && !target_reported {
                target_reported = true;
                uart_puts(b"TARGET REACHED\r\n");
            }
            continue;
        }
        if hall == 0 || hall == 7 {
            invalid_hall += 1;
            if invalid_hall > 20_000 {
                fault(b"FAULT HALL\r\n");
            }
        } else {
            invalid_hall = 0;
            write(TIM1_CCER, COMMUTATION[hall as usize]);
            if hall != previous_hall {
                previous_hall = hall;
                edges += 1;
                stationary = 0;
                // Bounded P regulator: short edge period means faster motor.
                let error = 14_000i32 - since_edge.min(28_000) as i32;
                duty = (duty as i32 + error / 256).clamp(MIN_DUTY as i32, MAX_DUTY as i32) as u32;
                write(TIM1_CCR1, duty);
                write(TIM1_CCR2, duty);
                write(TIM1_CCR3, duty);
                since_edge = 0;
                if edges == 8 && !target_reported {
                    target_reported = true;
                    uart_puts(b"TARGET REACHED\r\n");
                }
            } else {
                stationary += 1;
                if edges >= 8 && stationary > 4_000_000 {
                    fault(b"FAULT STALL\r\n");
                }
            }
        }
        since_edge = since_edge.saturating_add(1);
    }
}
