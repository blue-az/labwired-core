// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! End-to-end waveform test for an STM32 USART: route PB10 to AF7 exactly as a
//! HAL MSP init does, arm the in-engine logic analyzer on it, transmit through
//! TDR, and assert the captured edges decode back to the characters sent.
//!
//! PB10 is USART3_TX (DS10198 Table 17) — a pad a user actually probes when
//! they ask "is my board printing?". It carried nothing before: serial output
//! existed as console text only, and the pad read as the idle GPIO latch. Its
//! AF nibble also lives in AFR**H** rather than AFRL, which is the half of the
//! selector decode a pad below 8 never reaches.
//!
//! USART3 and GPIO**B** rather than instance 1 and GPIOA: the default test bus
//! already ships a `uart1` and an F1-layout `gpioa`, and a second peripheral of
//! either name is shadowed by it rather than replacing it — the pad routes bind
//! to the pre-existing instance and the new one narrates to nothing.
//!
//! The decoder shares no code with the model — it synchronises on the falling
//! start edge and samples each bit at its centre, LSB first, as a receiver
//! does. The waveform reaches it through the normal `read_gpio_pad` /
//! pad-route path; nothing is synthesized into the capture ring by the test.

#[cfg(test)]
mod stm32_uart_waveform_tests {
    use crate::cpu::CortexM;
    use crate::logic_capture::LogicEdge;
    use crate::peripherals::gpio::{GpioPort, GpioRegisterLayout};
    use crate::peripherals::uart::{Uart, UartRegisterLayout};
    use crate::{Bus, Machine};

    const RAM_BASE: u64 = 0x2000_0000;
    const GPIOB_BASE: u64 = 0x4001_0C00;
    const USART3_BASE: u64 = 0x4000_4800;

    const MODER: u64 = 0x00;
    const AFRL: u64 = 0x20;
    const AFRH: u64 = 0x24;

    /// USARTv2 register map: BRR holds USARTDIV, TDR transmits.
    const BRR: u64 = 0x0C;
    const TDR: u64 = 0x28;

    /// PB10 = USART3_TX on AF7 (DS10198 Table 17).
    const TX_PIN: u8 = 10;
    /// PB3 carries no USART function at all — the control case.
    const NON_UART_PIN: u8 = 3;
    const CH_TX: u32 = 0;

    /// 115200 baud from an 80 MHz PCLK with OVER16: USARTDIV = 80e6 / 115200 =
    /// 694 (0x2B6), and the divisor IS one bit period in peripheral clocks.
    const USARTDIV: u32 = 694;
    const BIT_TIME: u64 = 694;

    fn machine() -> Machine<CortexM> {
        let mut bus = crate::bus::SystemBus::new();
        let (cpu, _nvic) = crate::system::cortex_m::configure_cortex_m(&mut bus);
        // A V2 GPIOB: the AFRL nibble is what routes the pad to the USART.
        bus.add_peripheral(
            "gpiob",
            GPIOB_BASE,
            0x400,
            None,
            Box::new(GpioPort::new_with_layout(GpioRegisterLayout::Stm32V2)),
        );
        let mut uart = Uart::new_with_layout(UartRegisterLayout::Stm32V2);
        // Keep the test's own stdout clean; the pad waveform is what is asserted.
        uart.set_sink(None, false);
        bus.add_peripheral("usart3", USART3_BASE, 0x400, None, Box::new(uart));
        bus.wire_stm32_uart_pads();

        let mut machine = Machine::new(cpu, bus);
        // NOP slab (`movs r0, #0`) with a Thumb `b` back to the start, so
        // `step()` advances cycles deterministically.
        for i in 0..1022u64 {
            let byte = if i % 2 == 0 { 0x00 } else { 0x20 };
            machine.bus.write_u8(RAM_BASE + i, byte).unwrap();
        }
        machine.bus.write_u8(RAM_BASE + 1022, 0xFF).unwrap();
        machine.bus.write_u8(RAM_BASE + 1023, 0xE5).unwrap();
        machine.cpu.pc = RAM_BASE as u32;
        machine
    }

    /// Put PB10 in alternate-function mode on AF7 and set the baud divisor, as
    /// the HAL's MSP init plus `UART_SetConfig` do.
    fn configure(machine: &mut Machine<CortexM>, pin: u8) {
        let bus = &mut machine.bus;
        bus.write_u32(GPIOB_BASE + MODER, 0b10 << (pin * 2))
            .unwrap();
        // Pads 8..15 select their AF through AFRH, indexed from pin 8.
        bus.write_u32(GPIOB_BASE + AFRH, 7 << ((pin - 8) * 4))
            .unwrap();
        bus.write_u32(USART3_BASE + BRR, USARTDIV).unwrap();
    }

    /// Write the characters into TDR, then let the engine run long enough for
    /// the line to have carried them — ten bit periods per character.
    fn transmit(machine: &mut Machine<CortexM>, text: &[u8]) {
        for &byte in text {
            machine.bus.write_u8(USART3_BASE + TDR, byte).unwrap();
        }
        for _ in 0..text.len() as u64 * 10 * BIT_TIME + 16 {
            machine.step().unwrap();
        }
    }

    /// An INDEPENDENT asynchronous-serial decoder: sync on the falling start
    /// edge, sample each bit at its centre, LSB first.
    fn decode(edges: &[LogicEdge], bit_time: u64) -> Vec<u8> {
        let timeline: Vec<(u64, bool)> = edges
            .iter()
            .filter(|e| e.ch == CH_TX)
            .map(|e| (e.cycle, e.value))
            .collect();
        let level_at = |t: u64| -> bool {
            timeline
                .iter()
                .rev()
                .find(|(cycle, _)| *cycle <= t)
                .map(|(_, level)| *level)
                .unwrap_or(true)
        };

        let mut bytes = Vec::new();
        let mut cursor = 0u64;
        for &(cycle, level) in &timeline {
            if level || cycle < cursor {
                continue;
            }
            if level_at(cycle + bit_time / 2) {
                continue; // a glitch, not a start bit
            }
            let mut byte = 0u8;
            for index in 0..8u64 {
                if level_at(cycle + bit_time / 2 + bit_time * (index + 1)) {
                    byte |= 1 << index; // LSB first
                }
            }
            if !level_at(cycle + bit_time / 2 + bit_time * 9) {
                continue; // no stop bit: not a character
            }
            bytes.push(byte);
            cursor = cycle + bit_time * 10;
        }
        bytes
    }

    #[test]
    fn logic_capture_sees_a_decodable_stm32_uart_waveform() {
        let mut machine = machine();
        configure(&mut machine, TX_PIN);

        let gpio_idx = machine.bus.find_peripheral_index_by_name("gpiob").unwrap();
        let initial = machine.logic_watch(&[Some((gpio_idx, TX_PIN))]);
        assert_eq!(
            initial,
            vec![Some(true)],
            "an idle serial line rests at mark, so a start bit is a falling edge",
        );

        for _ in 0..20_000 {
            machine.step().unwrap();
        }
        transmit(&mut machine, b"Hi!\n");

        let edges = machine.logic_read_edges(0).edges;
        assert!(
            !edges.is_empty(),
            "a USART transmission must put edges on its AF-routed pad",
        );
        assert_eq!(
            decode(&edges, BIT_TIME),
            b"Hi!\n".to_vec(),
            "the wire must carry the characters the firmware transmitted",
        );
    }

    #[test]
    fn a_pad_that_carries_no_usart_function_shows_no_serial_traffic() {
        // This gates the TABLE, not the AF mode: PB3 is put into alternate
        // function 7 exactly like the real TX pad, and must still stay silent,
        // because AF7 on PB3 is not a USART function at all. Route a pad the
        // datasheet does not list and this is what catches it.
        let mut machine = machine();
        configure(&mut machine, TX_PIN);
        machine
            .bus
            .write_u32(GPIOB_BASE + MODER, 0b10 << (NON_UART_PIN * 2))
            .unwrap();
        machine
            .bus
            .write_u32(GPIOB_BASE + AFRL, 7 << (NON_UART_PIN * 4))
            .unwrap();

        let gpio_idx = machine.bus.find_peripheral_index_by_name("gpiob").unwrap();
        machine.logic_watch(&[Some((gpio_idx, NON_UART_PIN))]);
        for _ in 0..20_000 {
            machine.step().unwrap();
        }
        transmit(&mut machine, b"Hi!\n");

        assert!(
            machine.logic_read_edges(0).edges.is_empty(),
            "AF7 on a pad with no USART function must not show the serial line",
        );
    }

    #[test]
    fn the_line_runs_at_the_baud_rate_brr_programs() {
        let mut machine = machine();
        configure(&mut machine, TX_PIN);
        let gpio_idx = machine.bus.find_peripheral_index_by_name("gpiob").unwrap();
        machine.logic_watch(&[Some((gpio_idx, TX_PIN))]);
        for _ in 0..20_000 {
            machine.step().unwrap();
        }
        // 0x55 alternates on every bit, so the gaps between transitions ARE the
        // bit period.
        transmit(&mut machine, &[0x55]);

        let cycles: Vec<u64> = machine
            .logic_read_edges(0)
            .edges
            .iter()
            .filter(|e| e.ch == CH_TX)
            .map(|e| e.cycle)
            .collect();
        assert!(
            cycles.len() >= 9,
            "0x55 alternates on every bit: {cycles:?}"
        );
        let gaps: Vec<u64> = cycles.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert!(
            gaps.iter().all(|&g| g == BIT_TIME),
            "every bit should last BRR = {BIT_TIME} cycles, got {gaps:?}",
        );
    }

    #[test]
    fn a_usart_whose_baud_was_never_programmed_publishes_nothing() {
        // BRR reads 0 out of reset. Narrating at a made-up rate would give a
        // trace that measures a frequency the firmware never asked for.
        let mut machine = machine();
        let bus = &mut machine.bus;
        bus.write_u32(GPIOB_BASE + MODER, 0b10 << (TX_PIN * 2))
            .unwrap();
        bus.write_u32(GPIOB_BASE + AFRH, 7 << ((TX_PIN - 8) * 4))
            .unwrap();

        let gpio_idx = machine.bus.find_peripheral_index_by_name("gpiob").unwrap();
        machine.logic_watch(&[Some((gpio_idx, TX_PIN))]);
        for _ in 0..20_000 {
            machine.step().unwrap();
        }
        transmit(&mut machine, b"Hi!\n");

        assert!(
            machine.logic_read_edges(0).edges.is_empty(),
            "no programmed baud rate means no honest timebase, so no waveform",
        );
    }
}
