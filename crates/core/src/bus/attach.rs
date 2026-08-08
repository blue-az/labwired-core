// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! I2C/SPI attach funnels and chip pad wiring.

use super::*;

impl SystemBus {
    /// Attach an I²C slave without a physical route. This remains suitable for
    /// fixed-pin controllers and low-level test fixtures; ESP32-C3 rejects it
    /// because C3's GPIO matrix makes a controller-only binding ambiguous.
    pub fn attach_i2c_slave(
        &mut self,
        controller: &str,
        dev: Box<dyn crate::peripherals::i2c::I2cDevice>,
    ) -> anyhow::Result<()> {
        self.attach_i2c_slave_with_route(controller, dev, None)
    }

    /// The single funnel through which every manifest-backed I²C slave reaches
    /// a controller. `route` is a target-neutral signal map (`sda`/`scl` for
    /// I²C); ESP32-C3 lowers it to real GPIO-matrix pads and rejects missing,
    /// unsupported, or ambiguous routes instead of silently attaching by bus
    /// name alone. Other controller families preserve the generic shape for
    /// forward-compatible physical routing while retaining their fixed-pin
    /// behavior today.
    pub fn attach_i2c_slave_with_route(
        &mut self,
        controller: &str,
        dev: Box<dyn crate::peripherals::i2c::I2cDevice>,
        route: Option<&std::collections::BTreeMap<String, String>>,
    ) -> anyhow::Result<()> {
        let wrapped = bus_trace::wrap_i2c(controller, &self.bus_trace, dev);
        let idx = self
            .find_peripheral_index_by_name(controller)
            .ok_or_else(|| anyhow::anyhow!("attach_i2c_slave: no peripheral '{controller}'"))?;
        let any = self.peripherals[idx].dev.as_any_mut().ok_or_else(|| {
            anyhow::anyhow!("attach_i2c_slave: '{controller}' is not downcastable")
        })?;
        if let Some(c) = any.downcast_mut::<crate::peripherals::i2c::I2c>() {
            c.push_slave(wrapped);
        } else if let Some(c) = any.downcast_mut::<crate::peripherals::esp32c3::i2c::Esp32c3I2c>() {
            let route = route.ok_or_else(|| {
                anyhow::anyhow!(
                    "ESP32-C3 I2C external device on '{controller}' requires both route.sda and route.scl"
                )
            })?;
            let route =
                crate::peripherals::esp32c3::i2c::C3I2cPadRoute::from_manifest_route(route)?;
            c.push_slave_with_route(wrapped, route);
        } else if let Some(c) = any.downcast_mut::<crate::peripherals::esp32s3::i2c::Esp32s3I2c>() {
            c.push_slave(wrapped);
        } else if let Some(c) = any.downcast_mut::<crate::peripherals::esp32::i2c::Esp32I2c>() {
            c.push_slave(wrapped);
        } else if let Some(c) = any.downcast_mut::<crate::peripherals::nrf52::twim::Nrf52Twim>() {
            c.push_slave(wrapped);
        } else if let Some(c) = any.downcast_mut::<crate::peripherals::nrf54l::twim::Nrf54lTwim>() {
            // Same kit → attach_i2c_device path as every other family. Without
            // this arm, smart-ring sensors could only reach the bus via the
            // nRF54L factory's build_i2c_tree loop — a second home for "what
            // does type X mean on this MCU".
            c.push_slave(wrapped);
        } else if let Some(c) =
            any.downcast_mut::<crate::peripherals::nrf52::serial_instance::Nrf52SerialInstance>()
        {
            // SPIM0/TWIM0 share one MMIO window; an I²C slave belongs to the
            // TWIM half. The nRF52 factory attaches manifest-declared externals
            // itself, but a programmatic attach to `i2c0` must land here too.
            c.attach_i2c(wrapped);
        } else if let Some(c) = any.downcast_mut::<crate::peripherals::rp2040::i2c::Rp2040I2c>() {
            c.push_slave(wrapped);
        } else {
            anyhow::bail!("attach_i2c_slave: '{controller}' is not an I2C controller");
        }
        Ok(())
    }

    /// Wire the ESP32-C3 I²C0 bit engine to C3 GPIO in both directions: GPIO
    /// reads the live SDA/SCL waveform, while I²C reads GPIO's live input/output
    /// matrix state before allowing a physically routed slave to acknowledge.
    /// No-op unless both C3 models are on the bus.
    pub(crate) fn wire_esp32c3_i2c_pads(&mut self) {
        use crate::peripherals::esp32c3::gpio::Esp32c3Gpio;
        use crate::peripherals::esp32c3::i2c::Esp32c3I2c;
        let i2c_idx = self.peripherals.iter().position(|p| {
            p.dev
                .as_any()
                .map(|a| a.is::<Esp32c3I2c>())
                .unwrap_or(false)
        });
        let gpio_idx = self.peripherals.iter().position(|p| {
            p.dev
                .as_any()
                .map(|a| a.is::<Esp32c3Gpio>())
                .unwrap_or(false)
        });
        let (Some(i2c_idx), Some(gpio_idx)) = (i2c_idx, gpio_idx) else {
            return;
        };
        let matrix_route = self.peripherals[gpio_idx]
            .dev
            .as_any()
            .and_then(|a| a.downcast_ref::<Esp32c3Gpio>())
            .map(|g| g.i2c_matrix_route_state());
        let lines = self.peripherals[i2c_idx]
            .dev
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<Esp32c3I2c>())
            .and_then(|c| {
                matrix_route.map(|route| {
                    c.set_matrix_route_state(route);
                    c.line_levels_arc()
                })
            });
        if let (Some(lines), Some(gpio)) = (
            lines,
            self.peripherals[gpio_idx]
                .dev
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<Esp32c3Gpio>()),
        ) {
            gpio.set_i2c_lines(lines);
        }
    }

    /// Bind the RP2040 UARTs' TX/RX wires to the pads IO_BANK0 can route them
    /// to, so a probe on GP0 shows the serial waveform rather than the SIO
    /// output latch. No-op unless IO_BANK0, SIO and a UART are all on the bus.
    ///
    /// The pad map is transcribed from the RP2040 SVD's `GPIOn_CTRL.FUNCSEL`
    /// enumerations (`uart0_tx`, `uart1_rx`, …) rather than derived, because it
    /// is not derivable: GP0–GP7 alternate instance every four pads, but GP8/GP9
    /// are UART**1**, not UART0, and any parity rule silently mis-assigns them.
    /// CTS/RTS pads carry no narrated waveform and are left out.
    pub(crate) fn wire_rp2040_uart_pads(&mut self) {
        use crate::peripherals::rp2040::io_bank0::{Rp2040IoBank0, GPIO_FUNC_UART};
        use crate::peripherals::rp2040::sio::Rp2040Sio;
        use crate::peripherals::uart::{Uart, LINE_RX, LINE_TX};

        /// `(pad, uart instance, line, function name)` — straight from the SVD.
        const PADS: &[(u8, usize, usize, &str)] = &[
            (0, 0, LINE_TX, "UART0_TX"),
            (1, 0, LINE_RX, "UART0_RX"),
            (4, 1, LINE_TX, "UART1_TX"),
            (5, 1, LINE_RX, "UART1_RX"),
            (8, 1, LINE_TX, "UART1_TX"),
            (9, 1, LINE_RX, "UART1_RX"),
            (12, 0, LINE_TX, "UART0_TX"),
            (13, 0, LINE_RX, "UART0_RX"),
            (16, 0, LINE_TX, "UART0_TX"),
            (17, 0, LINE_RX, "UART0_RX"),
            (20, 1, LINE_TX, "UART1_TX"),
            (21, 1, LINE_RX, "UART1_RX"),
            (24, 1, LINE_TX, "UART1_TX"),
            (25, 1, LINE_RX, "UART1_RX"),
            (28, 0, LINE_TX, "UART0_TX"),
            (29, 0, LINE_RX, "UART0_RX"),
        ];

        let Some(functions) = self
            .peripherals
            .iter()
            .find_map(|p| {
                p.dev
                    .as_any()
                    .and_then(|a| a.downcast_ref::<Rp2040IoBank0>())
            })
            .map(Rp2040IoBank0::pad_functions)
        else {
            return;
        };

        for (instance, name) in ["uart0", "uart1"].iter().enumerate() {
            let Some(idx) = self.find_peripheral_index_by_name(name) else {
                continue;
            };
            let Some(lines) = self.peripherals[idx]
                .dev
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<Uart>())
                .map(Uart::pad_lines_arc)
            else {
                continue;
            };
            let Some(sio_idx) = self.find_peripheral_index_by_name("sio") else {
                return;
            };
            let Some(sio) = self.peripherals[sio_idx]
                .dev
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<Rp2040Sio>())
            else {
                return;
            };
            for &(pin, pad_instance, line, func) in PADS {
                if pad_instance != instance {
                    continue;
                }
                sio.bind_pad_route(functions.clone(), &lines, pin, GPIO_FUNC_UART, line, func);
            }
        }
    }

    /// Bind the RP2040 I²C controllers' wires to the pads IO_BANK0 can route
    /// them to, so `read_gpio_pad` — and the logic analyzer through it — sees
    /// the bus rather than the SIO output latch. No-op unless IO_BANK0, SIO and
    /// an I²C controller are all on the bus.
    ///
    /// Pad assignment is the fixed RP2040 map (datasheet Table 2-19): with
    /// `FUNCSEL = GPIO_FUNC_I2C`, an EVEN pad carries SDA and an odd pad SCL,
    /// and the instance alternates every two pads — GP0/GP1 are I2C0, GP2/GP3
    /// are I2C1, GP4/GP5 are I2C0 again, and so on. Nothing here is chosen by
    /// us: which pads exist is the datasheet's, and which one is live at any
    /// moment is FUNCSEL's.
    pub(crate) fn wire_rp2040_i2c_pads(&mut self) {
        use crate::peripherals::rp2040::i2c::{Rp2040I2c, LINE_SCL, LINE_SDA};
        use crate::peripherals::rp2040::io_bank0::{Rp2040IoBank0, GPIO_FUNC_I2C, PAD_COUNT};
        use crate::peripherals::rp2040::sio::Rp2040Sio;

        let Some(functions) = self
            .peripherals
            .iter()
            .find_map(|p| {
                p.dev
                    .as_any()
                    .and_then(|a| a.downcast_ref::<Rp2040IoBank0>())
            })
            .map(Rp2040IoBank0::pad_functions)
        else {
            return;
        };

        for (instance, name) in ["i2c0", "i2c1"].iter().enumerate() {
            let Some(idx) = self.find_peripheral_index_by_name(name) else {
                continue;
            };
            let Some(lines) = self.peripherals[idx]
                .dev
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<Rp2040I2c>())
                .map(Rp2040I2c::pad_lines_arc)
            else {
                continue;
            };
            let Some(sio_idx) = self.find_peripheral_index_by_name("sio") else {
                return;
            };
            let Some(sio) = self.peripherals[sio_idx]
                .dev
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<Rp2040Sio>())
            else {
                return;
            };
            for pin in 0..PAD_COUNT {
                if usize::from(pin / 2) % 2 != instance {
                    continue;
                }
                let (line, func) = if pin % 2 == 0 {
                    (
                        LINE_SDA,
                        if instance == 0 {
                            "I2C0_SDA"
                        } else {
                            "I2C1_SDA"
                        },
                    )
                } else {
                    (
                        LINE_SCL,
                        if instance == 0 {
                            "I2C0_SCL"
                        } else {
                            "I2C1_SCL"
                        },
                    )
                };
                sio.bind_pad_route(functions.clone(), &lines, pin, GPIO_FUNC_I2C, line, func);
            }
        }
    }

    /// Share the ESP32-S3 I²C0 controller's live SCL/SDA levels with S3 GPIO,
    /// so pads whose output matrix routes `I2CEXT0_SCL`/`SDA` read the real
    /// waveform through `read_gpio_pad` (which is what the in-engine logic
    /// analyzer samples). No-op unless both S3 models are on the bus.
    ///
    /// The C3 counterpart is [`Self::wire_esp32c3_i2c_pads`]; unlike the C3
    /// this direction is one-way, because the S3 I²C model resolves its slaves
    /// by address rather than by physical pad route.
    pub(crate) fn wire_esp32s3_i2c_pads(&mut self) {
        use crate::peripherals::esp32s3::gpio::Esp32s3Gpio;
        use crate::peripherals::esp32s3::i2c::Esp32s3I2c;

        let i2c_idx = self.peripherals.iter().position(|p| {
            p.dev
                .as_any()
                .map(|a| a.is::<Esp32s3I2c>())
                .unwrap_or(false)
        });
        let gpio_idx = self.peripherals.iter().position(|p| {
            p.dev
                .as_any()
                .map(|a| a.is::<Esp32s3Gpio>())
                .unwrap_or(false)
        });
        let (Some(i2c_idx), Some(gpio_idx)) = (i2c_idx, gpio_idx) else {
            return;
        };
        let lines = self.peripherals[i2c_idx]
            .dev
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<Esp32s3I2c>())
            .map(|c| c.pad_lines_arc());
        if let (Some(lines), Some(gpio)) = (
            lines,
            self.peripherals[gpio_idx]
                .dev
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<Esp32s3Gpio>()),
        ) {
            gpio.set_i2c_lines(lines);
        }
    }

    /// Wire C3 IO_MUX per-pad controls into C3 GPIO after both models have
    /// been constructed. The IO_MUX owns the shared register bank; GPIO reads
    /// `FUN_WPU` from it to model Arduino `INPUT_PULLUP`. No-op on any bus
    /// without both C3 peripherals.
    pub(crate) fn wire_esp32c3_pad_controls(&mut self) {
        use crate::peripherals::esp32c3::gpio::Esp32c3Gpio;
        use crate::peripherals::esp32c3::io_mux::Esp32c3IoMux;

        let io_mux_idx = self.peripherals.iter().position(|p| {
            p.dev
                .as_any()
                .map(|any| any.is::<Esp32c3IoMux>())
                .unwrap_or(false)
        });
        let gpio_idx = self.peripherals.iter().position(|p| {
            p.dev
                .as_any()
                .map(|any| any.is::<Esp32c3Gpio>())
                .unwrap_or(false)
        });
        let (Some(io_mux_idx), Some(gpio_idx)) = (io_mux_idx, gpio_idx) else {
            return;
        };

        let controls = self.peripherals[io_mux_idx]
            .dev
            .as_any()
            .and_then(|any| any.downcast_ref::<Esp32c3IoMux>())
            .map(Esp32c3IoMux::pad_controls);
        if let (Some(controls), Some(gpio)) = (
            controls,
            self.peripherals[gpio_idx]
                .dev
                .as_any_mut()
                .and_then(|any| any.downcast_mut::<Esp32c3Gpio>()),
        ) {
            gpio.set_pad_controls(controls);
        }
    }

    /// Bracket a C3 IO_MUX write with GPIO push-capture sampling. A `FUN_WPU`
    /// write changes an input pad electrically even though the GPIO register
    /// block itself is not written, so the usual GPIO-local write hooks would
    /// otherwise miss the edge. The returned GPIO index is passed to
    /// [`Self::finish_esp32c3_io_mux_write`] after the MMIO write succeeds.
    pub(crate) fn begin_esp32c3_io_mux_write(&mut self, io_mux_idx: usize) -> Option<usize> {
        use crate::peripherals::esp32c3::gpio::Esp32c3Gpio;
        use crate::peripherals::esp32c3::io_mux::Esp32c3IoMux;

        if !self.peripherals.get(io_mux_idx).is_some_and(|p| {
            p.dev
                .as_any()
                .map(|any| any.is::<Esp32c3IoMux>())
                .unwrap_or(false)
        }) {
            return None;
        }
        let gpio_idx = self.peripherals.iter().position(|p| {
            p.dev
                .as_any()
                .map(|any| any.is::<Esp32c3Gpio>())
                .unwrap_or(false)
        })?;
        self.peripherals[gpio_idx]
            .dev
            .as_any_mut()
            .and_then(|any| any.downcast_mut::<Esp32c3Gpio>())?
            .tap_snapshot();
        Some(gpio_idx)
    }

    /// Complete a successful C3 IO_MUX write started by
    /// [`Self::begin_esp32c3_io_mux_write`], pushing any changed pad level to
    /// the in-engine logic tap.
    pub(crate) fn finish_esp32c3_io_mux_write(&mut self, gpio_idx: Option<usize>) {
        let Some(gpio_idx) = gpio_idx else {
            return;
        };
        if let Some(gpio) = self.peripherals[gpio_idx]
            .dev
            .as_any_mut()
            .and_then(|any| any.downcast_mut::<crate::peripherals::esp32c3::gpio::Esp32c3Gpio>())
        {
            gpio.tap_report();
        }
    }

    /// Wire the STM32 SPI bit engines' live SCK/MOSI/MISO levels into the
    /// STM32 GPIO ports, so pads whose MODER/AFR (V2) or CRL/CRH CNF (F1)
    /// route an SPI alternate function read the real waveform through
    /// `read_gpio_pad` (which is what the in-engine logic analyzer samples).
    /// The SPI counterpart of [`Self::wire_esp32c3_i2c_pads`]; no-op on buses
    /// without a classic/FIFO STM32 SPI.
    ///
    /// Signal mapping comes from static per-family AF tables sourced from the
    /// datasheet alternate-function maps:
    /// * L4 (FIFO SPI + V2 GPIO): STM32L476 datasheet DS10198 Table 17 —
    ///   SPI1/SPI2 on AF5, SPI3 on AF6.
    /// * F4 (classic SPI + V2 GPIO): STM32F407 datasheet DS8626 Table 9 —
    ///   SPI1/SPI2 on AF5.
    /// * F1 (classic SPI + F1 GPIO): RM0008 §9.3 default pinout, no AFIO
    ///   remap (remap is not modeled). F1 MISO pads are input-mode on real
    ///   silicon and are intentionally not routed (see `GpioPort` docs).
    pub(crate) fn wire_stm32_spi_pads(&mut self) {
        use crate::peripherals::gpio::{GpioPort, GpioRegisterLayout};
        use crate::peripherals::spi::{Spi, SpiSignal};
        use SpiSignal::{Miso, Mosi, Sck};

        // (spi, port, pin, AF, signal, func) — V2 ports, L4 parts (DS10198
        // Table 17: SPI1-3).
        const L4: &[(&str, char, u8, u8, SpiSignal, &str)] = &[
            ("spi1", 'a', 5, 5, Sck, "SPI1_SCK"),
            ("spi1", 'a', 6, 5, Miso, "SPI1_MISO"),
            ("spi1", 'a', 7, 5, Mosi, "SPI1_MOSI"),
            ("spi1", 'b', 3, 5, Sck, "SPI1_SCK"),
            ("spi1", 'b', 4, 5, Miso, "SPI1_MISO"),
            ("spi1", 'b', 5, 5, Mosi, "SPI1_MOSI"),
            ("spi1", 'e', 13, 5, Sck, "SPI1_SCK"),
            ("spi1", 'e', 14, 5, Miso, "SPI1_MISO"),
            ("spi1", 'e', 15, 5, Mosi, "SPI1_MOSI"),
            ("spi2", 'b', 10, 5, Sck, "SPI2_SCK"),
            ("spi2", 'b', 13, 5, Sck, "SPI2_SCK"),
            ("spi2", 'b', 14, 5, Miso, "SPI2_MISO"),
            ("spi2", 'b', 15, 5, Mosi, "SPI2_MOSI"),
            ("spi2", 'c', 2, 5, Miso, "SPI2_MISO"),
            ("spi2", 'c', 3, 5, Mosi, "SPI2_MOSI"),
            ("spi2", 'd', 1, 5, Sck, "SPI2_SCK"),
            ("spi2", 'd', 3, 5, Miso, "SPI2_MISO"),
            ("spi2", 'd', 4, 5, Mosi, "SPI2_MOSI"),
            ("spi3", 'b', 3, 6, Sck, "SPI3_SCK"),
            ("spi3", 'b', 4, 6, Miso, "SPI3_MISO"),
            ("spi3", 'b', 5, 6, Mosi, "SPI3_MOSI"),
            ("spi3", 'c', 10, 6, Sck, "SPI3_SCK"),
            ("spi3", 'c', 11, 6, Miso, "SPI3_MISO"),
            ("spi3", 'c', 12, 6, Mosi, "SPI3_MOSI"),
        ];
        // V2 ports, F4 parts (DS8626 Table 9: SPI1-2).
        const F4: &[(&str, char, u8, u8, SpiSignal, &str)] = &[
            ("spi1", 'a', 5, 5, Sck, "SPI1_SCK"),
            ("spi1", 'a', 6, 5, Miso, "SPI1_MISO"),
            ("spi1", 'a', 7, 5, Mosi, "SPI1_MOSI"),
            ("spi1", 'b', 3, 5, Sck, "SPI1_SCK"),
            ("spi1", 'b', 4, 5, Miso, "SPI1_MISO"),
            ("spi1", 'b', 5, 5, Mosi, "SPI1_MOSI"),
            ("spi2", 'b', 10, 5, Sck, "SPI2_SCK"),
            ("spi2", 'b', 13, 5, Sck, "SPI2_SCK"),
            ("spi2", 'b', 14, 5, Miso, "SPI2_MISO"),
            ("spi2", 'b', 15, 5, Mosi, "SPI2_MOSI"),
            ("spi2", 'c', 2, 5, Miso, "SPI2_MISO"),
            ("spi2", 'c', 3, 5, Mosi, "SPI2_MOSI"),
        ];
        // F1 ports (RM0008 §9.3 default mapping, SPI1-2, SCK/MOSI only).
        const F1: &[(&str, char, u8, SpiSignal, &str)] = &[
            ("spi1", 'a', 5, Sck, "SPI1_SCK"),
            ("spi1", 'a', 7, Mosi, "SPI1_MOSI"),
            ("spi2", 'b', 13, Sck, "SPI2_SCK"),
            ("spi2", 'b', 15, Mosi, "SPI2_MOSI"),
        ];

        for spi_name in ["spi1", "spi2", "spi3"] {
            let Some(spi_idx) = self.find_peripheral_index_by_name(spi_name) else {
                continue;
            };
            let Some((fifo, lines)) = self.peripherals[spi_idx]
                .dev
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<Spi>())
                .filter(|s| s.is_stm32_wire_layout())
                .map(|s| (s.is_fifo_layout(), s.line_levels_arc()))
            else {
                continue;
            };
            for port in ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'] {
                let Some(gpio_idx) = self.find_peripheral_index_by_name(&format!("gpio{port}"))
                else {
                    continue;
                };
                let Some(gpio) = self.peripherals[gpio_idx]
                    .dev
                    .as_any_mut()
                    .and_then(|a| a.downcast_mut::<GpioPort>())
                else {
                    continue;
                };
                match gpio.register_layout() {
                    GpioRegisterLayout::Stm32V2 => {
                        let table = if fifo { L4 } else { F4 };
                        for &(spi, p, pin, af, sig, func) in table {
                            if spi == spi_name && p == port {
                                gpio.add_pad_route(
                                    lines.pad_lines(),
                                    pin,
                                    Some(af),
                                    sig as usize,
                                    func,
                                );
                            }
                        }
                    }
                    GpioRegisterLayout::Stm32F1 => {
                        for &(spi, p, pin, sig, func) in F1 {
                            if spi == spi_name && p == port {
                                gpio.add_pad_route(
                                    lines.pad_lines(),
                                    pin,
                                    None,
                                    sig as usize,
                                    func,
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Route each STM32 I²C controller's SCL/SDA onto the GPIO pads that can
    /// carry them, so `read_gpio_pad` — and the logic analyzer through it —
    /// sees the wire this controller drives rather than the idle GPIO latch.
    ///
    /// The SPI counterpart is [`Self::wire_stm32_spi_pads`]; both install
    /// routes through the one `add_pad_route` mechanism, differing only in
    /// their AF table.
    pub(crate) fn wire_stm32_i2c_pads(&mut self) {
        use crate::peripherals::gpio::{GpioPort, GpioRegisterLayout};
        use crate::peripherals::i2c::{I2c, LINE_SCL, LINE_SDA};

        // (i2c, port, pin, AF, line, func). L4: DS10198 Table 17 (I2C1-3 on
        // AF4, and I2C4 on AF5 where fitted — not modelled here). F4: DS8626
        // Table 9 gives the same AF4 assignment for I2C1-3, and the pins below
        // are common to both, so one V2 table serves them.
        const V2: &[(&str, char, u8, u8, usize, &str)] = &[
            ("i2c1", 'b', 6, 4, LINE_SCL, "I2C1_SCL"),
            ("i2c1", 'b', 7, 4, LINE_SDA, "I2C1_SDA"),
            ("i2c1", 'b', 8, 4, LINE_SCL, "I2C1_SCL"),
            ("i2c1", 'b', 9, 4, LINE_SDA, "I2C1_SDA"),
            ("i2c2", 'b', 10, 4, LINE_SCL, "I2C2_SCL"),
            ("i2c2", 'b', 11, 4, LINE_SDA, "I2C2_SDA"),
            ("i2c2", 'b', 13, 4, LINE_SCL, "I2C2_SCL"),
            ("i2c2", 'b', 14, 4, LINE_SDA, "I2C2_SDA"),
            ("i2c2", 'f', 0, 4, LINE_SDA, "I2C2_SDA"),
            ("i2c2", 'f', 1, 4, LINE_SCL, "I2C2_SCL"),
            ("i2c3", 'a', 7, 4, LINE_SCL, "I2C3_SCL"),
            ("i2c3", 'b', 4, 4, LINE_SDA, "I2C3_SDA"),
            ("i2c3", 'c', 0, 4, LINE_SCL, "I2C3_SCL"),
            ("i2c3", 'c', 1, 4, LINE_SDA, "I2C3_SDA"),
            ("i2c3", 'c', 9, 4, LINE_SDA, "I2C3_SDA"),
        ];

        for i2c_name in ["i2c1", "i2c2", "i2c3"] {
            let Some(i2c_idx) = self.find_peripheral_index_by_name(i2c_name) else {
                continue;
            };
            let Some(lines) = self.peripherals[i2c_idx]
                .dev
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<I2c>())
                .and_then(|i2c| i2c.pad_lines_arc())
            else {
                continue;
            };
            for port in ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'] {
                let Some(gpio_idx) = self.find_peripheral_index_by_name(&format!("gpio{port}"))
                else {
                    continue;
                };
                let Some(gpio) = self.peripherals[gpio_idx]
                    .dev
                    .as_any_mut()
                    .and_then(|a| a.downcast_mut::<GpioPort>())
                else {
                    continue;
                };
                // F1's I²C sits on a different controller model (F1I2c) and its
                // pads are open-drain AF on the fixed mapping; only the V2
                // register model is routed here.
                if gpio.register_layout() != GpioRegisterLayout::Stm32V2 {
                    continue;
                }
                for &(i2c, p, pin, af, line, func) in V2 {
                    if i2c == i2c_name && p == port {
                        gpio.add_pad_route(&lines, pin, Some(af), line, func);
                    }
                }
            }
        }
    }

    /// Route each STM32 USART's TX/RX onto the GPIO pads that can carry them,
    /// so a probe shows the serial waveform rather than the idle GPIO latch.
    ///
    /// Same mechanism as [`Self::wire_stm32_i2c_pads`] and
    /// [`Self::wire_stm32_spi_pads`] — one `add_pad_route` per (pad, AF), and
    /// the AF nibble decides which is live. Only the table differs.
    pub(crate) fn wire_stm32_uart_pads(&mut self) {
        use crate::peripherals::gpio::{GpioPort, GpioRegisterLayout};
        use crate::peripherals::uart::{Uart, LINE_RX, LINE_TX};

        // (instance, port, pin, AF, line, func). Read out of the STM32L476
        // datasheet DS10198 Table 17 (AF0-AF7): USART1-3 sit on AF7 across the
        // V2 families. Only TX and RX are routed — CK/CTS/RTS carry no narrated
        // waveform.
        const V2: &[(u8, char, u8, u8, usize, &str)] = &[
            (1, 'a', 9, 7, LINE_TX, "USART1_TX"),
            (1, 'a', 10, 7, LINE_RX, "USART1_RX"),
            (1, 'b', 6, 7, LINE_TX, "USART1_TX"),
            (1, 'b', 7, 7, LINE_RX, "USART1_RX"),
            (1, 'g', 9, 7, LINE_TX, "USART1_TX"),
            (1, 'g', 10, 7, LINE_RX, "USART1_RX"),
            (2, 'a', 2, 7, LINE_TX, "USART2_TX"),
            (2, 'a', 3, 7, LINE_RX, "USART2_RX"),
            (2, 'd', 5, 7, LINE_TX, "USART2_TX"),
            (2, 'd', 6, 7, LINE_RX, "USART2_RX"),
            (3, 'b', 10, 7, LINE_TX, "USART3_TX"),
            (3, 'b', 11, 7, LINE_RX, "USART3_RX"),
            (3, 'c', 4, 7, LINE_TX, "USART3_TX"),
            (3, 'c', 5, 7, LINE_RX, "USART3_RX"),
            (3, 'c', 10, 7, LINE_TX, "USART3_TX"),
            (3, 'c', 11, 7, LINE_RX, "USART3_RX"),
            (3, 'd', 8, 7, LINE_TX, "USART3_TX"),
            (3, 'd', 9, 7, LINE_RX, "USART3_RX"),
        ];

        for instance in 1u8..=3 {
            // Chip configs name these both ways — `uart2` on the L4/F1 configs,
            // `usart2` on the G4. Looking up both is what stops a rename in one
            // yaml silently un-routing that chip's serial pads.
            let Some(uart_idx) = self
                .find_peripheral_index_by_name(&format!("uart{instance}"))
                .or_else(|| self.find_peripheral_index_by_name(&format!("usart{instance}")))
            else {
                continue;
            };
            let Some(lines) = self.peripherals[uart_idx]
                .dev
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<Uart>())
                .map(Uart::pad_lines_arc)
            else {
                continue;
            };
            for port in ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'] {
                let Some(gpio_idx) = self.find_peripheral_index_by_name(&format!("gpio{port}"))
                else {
                    continue;
                };
                let Some(gpio) = self.peripherals[gpio_idx]
                    .dev
                    .as_any_mut()
                    .and_then(|a| a.downcast_mut::<GpioPort>())
                else {
                    continue;
                };
                // The F1 selects alternate function through CRL/CRH rather than
                // an AF nibble, so its pads are a different decode — as with
                // I²C, only the V2 register model is routed here.
                if gpio.register_layout() != GpioRegisterLayout::Stm32V2 {
                    continue;
                }
                for &(inst, p, pin, af, line, func) in V2 {
                    if inst == instance && p == port {
                        gpio.add_pad_route(&lines, pin, Some(af), line, func);
                    }
                }
            }
        }
    }

    /// The single funnel through which every SPI device reaches a controller —
    /// the SPI counterpart of [`Self::attach_i2c_slave`]. Wraps then dispatches.
    pub fn attach_spi_device(
        &mut self,
        controller: &str,
        dev: Box<dyn crate::peripherals::spi::SpiDevice>,
    ) -> anyhow::Result<()> {
        let wrapped = bus_trace::wrap_spi(controller, &self.bus_trace, dev);
        let idx = self
            .find_peripheral_index_by_name(controller)
            .ok_or_else(|| anyhow::anyhow!("attach_spi_device: no peripheral '{controller}'"))?;
        let any = self.peripherals[idx].dev.as_any_mut().ok_or_else(|| {
            anyhow::anyhow!("attach_spi_device: '{controller}' is not downcastable")
        })?;
        if let Some(c) = any.downcast_mut::<crate::peripherals::spi::Spi>() {
            c.push_device(wrapped);
        } else if let Some(c) = any.downcast_mut::<crate::peripherals::esp32c3::spi::Esp32c3Spi>() {
            c.push_device(wrapped);
        } else if let Some(c) = any.downcast_mut::<crate::peripherals::esp32::spi::Esp32Spi>() {
            c.push_device(wrapped);
        } else if let Some(c) = any.downcast_mut::<crate::peripherals::esp32s3::gpspi::Esp32s3Spi>()
        {
            c.push_device(wrapped);
        } else if let Some(c) =
            any.downcast_mut::<crate::peripherals::nrf52::serial_instance::Nrf52SerialInstance>()
        {
            // The SPIM half of the shared SPIM0/TWIM0 window.
            c.attach_spi(wrapped);
        } else {
            anyhow::bail!("attach_spi_device: '{controller}' is not a SPI controller");
        }
        Ok(())
    }
}
