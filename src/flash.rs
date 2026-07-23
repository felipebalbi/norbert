//! Hardware-agnostic iCE40HX8K-EVB SPI-NOR flashing core.

use core::fmt;
use std::time::Duration;
use embedded_hal::spi::{Operation, SpiDevice};

// Universal SPI-NOR opcodes (M25P16 / EN25QH16B / W25Q16).
const CMD_RDID: u8 = 0x9F;
const CMD_WREN: u8 = 0x06;
const CMD_RDSR: u8 = 0x05;
const CMD_READ: u8 = 0x03;
const CMD_PP:   u8 = 0x02;
const CMD_BE64: u8 = 0xD8; // 64 KiB block erase (M25P16-safe; no 0x20 4K erase!)
const CMD_CE:   u8 = 0xC7; // chip erase
const SR_WIP:   u8 = 0x01;

/// 256-byte program page.
pub const PAGE_SIZE:  usize = 256;
/// 64 KiB erase block.
pub const BLOCK_SIZE: usize = 64 * 1024;

/// JEDEC RDID (0x9F) response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlashId {
    pub manufacturer: u8,
    pub mem_type: u8,
    pub capacity_code: u8,
}

impl FlashId {
    /// Density in bytes = `2^capacity_code`; `None` for implausible codes.
    pub fn capacity_bytes(&self) -> Option<usize> {
        match self.capacity_code {
            0x10..=0x1B => Some(1usize << self.capacity_code),
            _ => None,
        }
    }

    /// True if a chip actually responded to RDID (an idle/floating bus reads
    /// all `0x00` or all `0xFF`).
    pub fn is_present(&self) -> bool {
        self.manufacturer != 0x00 && self.manufacturer != 0xFF
    }

    pub fn jedec(&self) -> [u8; 3] { [self.manufacturer, self.mem_type, self.capacity_code] }
}

impl fmt::Display for FlashId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mfr=0x{:02X} type=0x{:02X} cap=0x{:02X}",
            self.manufacturer, self.mem_type, self.capacity_code)?;
        if let Some(n) = self.capacity_bytes() {
            write!(f, " ({} KiB)", n / 1024)?;
        }
        Ok(())
    }
}

/// Holds another bus master off the shared SPI while we program, then releases it.
///
/// `acquire` must make the flash's SPI bus exclusively ours; `release` returns
/// control. For a bare chip / clip this is a no-op (`NoHold`). For a shared bus
/// it is a GPIO held at a level (the device layer's `HostBus`) — e.g. the iCE40 CRESET driven low
/// to tri-state the FPGA's SPI pins, then released Hi-Z so it reconfigures.
pub trait BusAccess {
    type Error: fmt::Debug;
    fn acquire(&mut self) -> Result<(), Self::Error>;
    fn release(&mut self) -> Result<(), Self::Error>;
}

/// No-op bus access for a bare flash (nothing else on the SPI bus). Default.
pub struct NoHold;
impl BusAccess for NoHold {
    type Error = core::convert::Infallible;
    fn acquire(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn release(&mut self) -> Result<(), Self::Error> { Ok(()) }
}

/// Progress callback events for the high-level flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Detecting,          // reading SFDP / choosing a flash profile (emitted once SFDP lands, Task 15)
    Erasing,
    Programming(usize), // bytes written so far
    Verifying(usize),   // bytes verified so far
    Done,
}

/// Flasher errors, generic over the SPI and bus-access error types.
#[derive(Debug)]
pub enum FlashError<S: fmt::Debug, R: fmt::Debug> {
    Spi(S),
    Bus(R),
    Timeout,
    VerifyMismatch { addr: usize, expected: u8, got: u8 },
    TooLarge { need: usize, have: usize },
    /// RDID read nothing — no chip on the bus.
    NoFlash,
    /// A chip responded but has no SFDP and no fallback-table entry.
    UnsupportedChip { jedec: [u8; 3] },
    /// A geometry op was attempted before `detect_profile` succeeded.
    NotDetected,
}

impl<S: fmt::Debug, R: fmt::Debug> fmt::Display for FlashError<S, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlashError::Spi(e) => write!(f, "SPI error: {e:?}"),
            FlashError::Bus(e) => write!(f, "bus-hold error: {e:?}"),
            FlashError::Timeout => write!(f, "timed out waiting for flash WIP to clear"),
            FlashError::VerifyMismatch { addr, expected, got } =>
                write!(f, "verify mismatch @0x{addr:06X}: expected 0x{expected:02X}, got 0x{got:02X}"),
            FlashError::TooLarge { need, have } =>
                write!(f, "image needs {need} bytes but flash is {have} bytes"),
            FlashError::NoFlash =>
                write!(f, "no SPI-NOR flash detected (RDID read all 0x00/0xFF)"),
            FlashError::UnsupportedChip { jedec } =>
                write!(f, "unsupported flash: JEDEC {jedec:02X?} has no SFDP and no fallback-table entry (add one)"),
            FlashError::NotDetected =>
                write!(f, "flash geometry unknown; run `detect` first"),
        }
    }
}
impl<S: fmt::Debug, R: fmt::Debug> std::error::Error for FlashError<S, R> {}

/// SPI-NOR flasher over an `embedded-hal` `SpiDevice` and a `BusAccess` line.
pub struct Flasher<SPI, RST> {
    spi: SPI,
    reset: RST,
    max_chunk: usize,
    poll_interval: Duration,
    poll_timeout: Duration,
}

impl<SPI, RST> Flasher<SPI, RST>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    /// Sensible defaults: 256-byte SPI chunks, 2 ms poll interval, 60 s timeout.
    pub fn new(spi: SPI, reset: RST) -> Self {
        Self::with_config(spi, reset, PAGE_SIZE, Duration::from_millis(2), Duration::from_secs(60))
    }

    /// `max_chunk` bounds bytes per USB SPI op; all writes/reads are split to it
    /// *within* a single CS transaction, so any value >= 4 is correct.
    pub fn with_config(
        spi: SPI, reset: RST, max_chunk: usize,
        poll_interval: Duration, poll_timeout: Duration,
    ) -> Self {
        assert!(max_chunk >= 4, "max_chunk must be >= 4");
        Self { spi, reset, max_chunk, poll_interval, poll_timeout }
    }

    fn spi_err(e: SPI::Error) -> FlashError<SPI::Error, RST::Error> { FlashError::Spi(e) }
    fn bus_err(e: RST::Error) -> FlashError<SPI::Error, RST::Error> { FlashError::Bus(e) }

    /// Read the JEDEC ID (0x9F).
    pub fn read_id(&mut self) -> Result<FlashId, FlashError<SPI::Error, RST::Error>> {
        let mut id = [0u8; 3];
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_RDID]), Operation::Read(&mut id)])
            .map_err(Self::spi_err)?;
        Ok(FlashId { manufacturer: id[0], mem_type: id[1], capacity_code: id[2] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use embedded_hal::spi::{Error as SpiErrorTrait, ErrorKind, ErrorType, Operation, SpiDevice};
    use std::cell::RefCell;
    use std::convert::Infallible;
    use std::rc::Rc;

    #[test]
    fn capacity_decode() {
        let id = FlashId { manufacturer: 0x20, mem_type: 0x20, capacity_code: 0x15 };
        assert_eq!(id.capacity_bytes(), Some(2 * 1024 * 1024)); // 16 Mbit = 2 MiB
        let bad = FlashId { manufacturer: 0, mem_type: 0, capacity_code: 0x00 };
        assert_eq!(bad.capacity_bytes(), None);
    }

    #[test]
    fn read_id_returns_jedec() {
        let flash = FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]);
        let mut f = flasher(flash, FakeBus::new(), 256);
        let id = f.read_id().unwrap();
        assert_eq!(id, FlashId { manufacturer: 0x20, mem_type: 0x20, capacity_code: 0x15 });
        assert_eq!(id.capacity_bytes(), Some(2 * 1024 * 1024));
    }

    #[derive(Debug)]
    pub struct FakeErr;
    impl SpiErrorTrait for FakeErr {
        fn kind(&self) -> ErrorKind { ErrorKind::Other }
    }

    struct FakeState {
        mem: Vec<u8>,
        id: [u8; 3],
        wel: bool,
        busy_reads: u32, // # of RDSR calls that report WIP=1 before clearing
    }

    /// Shareable behavioral SPI-NOR. Clone to inspect memory after moving one
    /// clone into a `Flasher`.
    #[derive(Clone)]
    struct FakeFlash(Rc<RefCell<FakeState>>);

    impl FakeFlash {
        fn new(size: usize, id: [u8; 3]) -> Self {
            FakeFlash(Rc::new(RefCell::new(FakeState {
                mem: vec![0xFF; size], id, wel: false, busy_reads: 0,
            })))
        }
        fn set_busy_reads(&self, n: u32) { self.0.borrow_mut().busy_reads = n; }
        fn mem(&self) -> Vec<u8> { self.0.borrow().mem.clone() }
        fn preset(&self, addr: usize, bytes: &[u8]) {
            self.0.borrow_mut().mem[addr..addr + bytes.len()].copy_from_slice(bytes);
        }
    }

    impl ErrorType for FakeFlash { type Error = FakeErr; }

    impl SpiDevice for FakeFlash {
        fn transaction(&mut self, ops: &mut [Operation<'_, u8>]) -> Result<(), FakeErr> {
            // Flatten MOSI bytes; collect MISO destinations in order.
            let mut mosi: Vec<u8> = Vec::new();
            let mut reads: Vec<&mut [u8]> = Vec::new();
            for op in ops.iter_mut() {
                match op {
                    Operation::Write(w) => mosi.extend_from_slice(w),
                    Operation::Read(r) => reads.push(r),
                    Operation::Transfer(r, w) => { mosi.extend_from_slice(w); reads.push(r); }
                    Operation::TransferInPlace(b) => { let c = b.to_vec(); mosi.extend_from_slice(&c); reads.push(b); }
                    Operation::DelayNs(_) => {}
                }
            }
            if mosi.is_empty() { return Ok(()); }

            let mut st = self.0.borrow_mut();
            let mut resp: Vec<u8> = Vec::new();
            let addr24 = |m: &[u8]| -> usize {
                u32::from_be_bytes([0, m[1], m[2], m[3]]) as usize
            };
            match mosi[0] {
                CMD_RDID => resp.extend_from_slice(&st.id),
                CMD_WREN => st.wel = true,
                CMD_RDSR => {
                    let wip = if st.busy_reads > 0 { st.busy_reads -= 1; SR_WIP } else { 0 };
                    let wel = if st.wel { 0x02 } else { 0 };
                    resp.push(wip | wel);
                }
                CMD_READ => {
                    let a = addr24(&mosi);
                    let total: usize = reads.iter().map(|r| r.len()).sum();
                    for i in 0..total { resp.push(*st.mem.get(a + i).unwrap_or(&0xFF)); }
                }
                CMD_PP => {
                    let a = addr24(&mosi);
                    if st.wel {
                        for (i, b) in mosi[4..].iter().enumerate() {
                            let page = a & !(PAGE_SIZE - 1);
                            let off = (a + i - page) % PAGE_SIZE; // wraps within page
                            st.mem[page + off] &= *b;              // NOR: program clears bits
                        }
                    }
                    st.wel = false;
                }
                CMD_BE64 => {
                    let a = addr24(&mosi);
                    if st.wel {
                        let base = a & !(BLOCK_SIZE - 1);
                        for b in &mut st.mem[base..base + BLOCK_SIZE] { *b = 0xFF; }
                    }
                    st.wel = false;
                }
                CMD_CE => { if st.wel { st.mem.iter_mut().for_each(|b| *b = 0xFF); } st.wel = false; }
                _ => {}
            }
            let mut ri = 0;
            for r in reads.iter_mut() {
                for b in r.iter_mut() { *b = *resp.get(ri).unwrap_or(&0xFF); ri += 1; }
            }
            Ok(())
        }
    }

    /// Shareable reset line; `true` = asserted (FPGA held in reset).
    #[derive(Clone)]
    struct FakeBus(Rc<RefCell<bool>>);
    impl FakeBus { fn new() -> Self { FakeBus(Rc::new(RefCell::new(false))) } fn asserted(&self) -> bool { *self.0.borrow() } }
    impl BusAccess for FakeBus {
        type Error = Infallible;
        fn acquire(&mut self) -> Result<(), Infallible> { *self.0.borrow_mut() = true; Ok(()) }
        fn release(&mut self) -> Result<(), Infallible> { *self.0.borrow_mut() = false; Ok(()) }
    }

    /// Fast Flasher over the fakes (zero poll interval so tests don't sleep).
    fn flasher(flash: FakeFlash, reset: FakeBus, max_chunk: usize)
        -> Flasher<FakeFlash, FakeBus>
    {
        Flasher::with_config(flash, reset, max_chunk, Duration::ZERO, Duration::from_secs(1))
    }
}
