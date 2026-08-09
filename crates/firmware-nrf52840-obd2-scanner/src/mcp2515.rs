//! MCP2515 over nRF52840 SPIM2 EasyDMA (16 MHz oscillator, 500 kbit/s CAN).

use crate::CanFrame;
use core::ptr::{read_volatile, write_volatile};

const SPIM: usize = 0x4002_3000;
const GPIO: usize = 0x5000_0000;
const CS: u32 = 1 << 12;
const WAIT_LIMIT: u32 = 100_000;
const RESET: u8 = 0xc0;
const READ: u8 = 0x03;
const WRITE: u8 = 0x02;
const BIT_MODIFY: u8 = 0x05;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Timeout,
    Configuration,
    Overflow,
    NoFrame,
}

pub struct Mcp2515;

impl Default for Mcp2515 {
    fn default() -> Self {
        Self::new()
    }
}
impl Mcp2515 {
    pub const fn new() -> Self {
        Self
    }

    pub fn init(&mut self) -> Result<(), Error> {
        unsafe {
            wr(GPIO, 0x518, CS);
            wr(GPIO, 0x508, CS);
            // IRQ P0.11 input with pull-up, CS output, SPI pins assigned directly.
            wr(GPIO, 0x700 + 11 * 4, 3 << 2);
            wr(SPIM, 0x508, 13);
            wr(SPIM, 0x50c, 14);
            wr(SPIM, 0x510, 15);
            wr(SPIM, 0x524, 0x0200_0000); // 2 MHz, safe for init and simulation.
            wr(SPIM, 0x554, 0);
            wr(SPIM, 0x500, 7);
        }
        self.command(&[RESET])?;
        // Configuration mode, 500 kbps at 16 MHz: BRP=0, 8 TQ/bit, SJW=1.
        self.write(0x0f, 0x80)?;
        self.write(0x2a, 0x00)?;
        self.write(0x29, 0x90)?;
        self.write(0x28, 0x02)?;
        // Exact 11-bit mask with filters for the functional ECU response ID.
        self.write_standard_id(0x20, 0x7ff)?; // RXM0
        self.write_standard_id(0x24, 0x7ff)?; // RXM1
        for filter in [0x00, 0x04, 0x08, 0x10, 0x14, 0x18] {
            self.write_standard_id(filter, 0x7e8)?;
        }
        self.write(0x60, 0x04)?; // standard-filtered, rollover enabled
        self.write(0x70, 0x00)?; // standard-filtered
        self.write(0x2b, 0x03)?;
        self.write(0x0f, 0x00)?;
        if self.read(0x0e)? & 0xe0 != 0 {
            return Err(Error::Configuration);
        }
        Ok(())
    }

    pub fn irq_asserted(&self) -> bool {
        unsafe { rd(GPIO, 0x510) & (1 << 11) == 0 }
    }
    pub fn interrupt_flags(&mut self) -> Result<u8, Error> {
        self.read(0x2c)
    }
    pub fn clear_overflow(&mut self) -> Result<(), Error> {
        self.bit_modify(0x1d, 0xc0, 0)
    }

    pub fn send(&mut self, frame: &CanFrame) -> Result<(), Error> {
        let id = frame.id & 0x7ff;
        let mut packet = [0u8; 14];
        packet[0] = 0x40;
        packet[1] = (id >> 3) as u8;
        packet[2] = (id << 5) as u8;
        packet[5] = frame.len.min(8);
        packet[6..14].copy_from_slice(&frame.data);
        self.command(&packet)?;
        self.command(&[0x81]) // RTS TX buffer 0
    }

    pub fn receive(&mut self) -> Result<CanFrame, Error> {
        let flags = self.interrupt_flags()?;
        let (opcode, clear) = if flags & 1 != 0 {
            (0x90, 1)
        } else if flags & 2 != 0 {
            (0x94, 2)
        } else {
            return Err(Error::NoFrame);
        };
        let tx = [opcode, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut rx = [0u8; 14];
        self.transfer(&tx, &mut rx)?;
        self.bit_modify(0x2c, clear, 0)?;
        let id = ((rx[1] as u16) << 3) | ((rx[2] as u16) >> 5);
        let len = rx[5] & 0x0f;
        if len > 8 {
            return Err(Error::Overflow);
        }
        let mut data = [0u8; 8];
        data.copy_from_slice(&rx[6..14]);
        Ok(CanFrame { id, len, data })
    }

    pub fn read(&mut self, address: u8) -> Result<u8, Error> {
        let mut rx = [0; 3];
        self.transfer(&[READ, address, 0], &mut rx)?;
        Ok(rx[2])
    }
    pub fn write(&mut self, address: u8, value: u8) -> Result<(), Error> {
        self.command(&[WRITE, address, value])
    }
    pub fn bit_modify(&mut self, address: u8, mask: u8, value: u8) -> Result<(), Error> {
        self.command(&[BIT_MODIFY, address, mask, value])
    }
    fn write_standard_id(&mut self, address: u8, id: u16) -> Result<(), Error> {
        self.command(&[WRITE, address, (id >> 3) as u8, (id << 5) as u8, 0, 0])
    }
    fn command(&mut self, tx: &[u8]) -> Result<(), Error> {
        let mut rx = [0u8; 14];
        self.transfer(tx, &mut rx[..tx.len()])
    }
    fn transfer(&mut self, tx: &[u8], rx: &mut [u8]) -> Result<(), Error> {
        unsafe {
            wr(GPIO, 0x50c, CS);
            wr(SPIM, 0x104, 0);
            wr(SPIM, 0x544, tx.as_ptr() as u32);
            wr(SPIM, 0x548, tx.len() as u32);
            wr(SPIM, 0x534, rx.as_mut_ptr() as u32);
            wr(SPIM, 0x538, rx.len() as u32);
            wr(SPIM, 0x010, 1);
            for _ in 0..WAIT_LIMIT {
                if rd(SPIM, 0x104) != 0 {
                    wr(GPIO, 0x508, CS);
                    return Ok(());
                }
            }
            wr(SPIM, 0x014, 1);
            wr(GPIO, 0x508, CS);
            Err(Error::Timeout)
        }
    }
}
unsafe fn wr(base: usize, offset: usize, value: u32) {
    write_volatile((base + offset) as *mut u32, value)
}
unsafe fn rd(base: usize, offset: usize) -> u32 {
    read_volatile((base + offset) as *const u32)
}
