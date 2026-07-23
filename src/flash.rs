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
    #[allow(dead_code)] // wired in Task 15 (SFDP detect: chip-presence check)
    pub fn is_present(&self) -> bool {
        self.manufacturer != 0x00 && self.manufacturer != 0xFF
    }

    #[allow(dead_code)] // wired in Task 15 (fallback-table lookup by JEDEC id)
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
#[allow(dead_code)] // library convenience (CLI always wires HostBus)
pub struct NoHold;
impl BusAccess for NoHold {
    type Error = core::convert::Infallible;
    fn acquire(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn release(&mut self) -> Result<(), Self::Error> { Ok(()) }
}

/// Progress callback events for the high-level flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    #[allow(dead_code)] // emitted in Task 15 (SFDP detect); main.rs only matches it
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
    #[allow(dead_code)] // constructed in Task 15 (detect)
    NoFlash,
    /// A chip responded but has no SFDP and no fallback-table entry.
    #[allow(dead_code)] // constructed in Task 15 (detect)
    UnsupportedChip { jedec: [u8; 3] },
    /// A geometry op was attempted before `detect_profile` succeeded.
    #[allow(dead_code)] // constructed in Task 15 (detect)
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
    #[allow(dead_code)] // library convenience (CLI uses with_config)
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

    fn write_enable(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.spi.transaction(&mut [Operation::Write(&[CMD_WREN])]).map_err(Self::spi_err)
    }

    fn read_status(&mut self) -> Result<u8, FlashError<SPI::Error, RST::Error>> {
        let mut sr = [0u8; 1];
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_RDSR]), Operation::Read(&mut sr)])
            .map_err(Self::spi_err)?;
        Ok(sr[0])
    }

    /// Poll RDSR until WIP clears, sleeping `poll_interval` between polls.
    fn wait_ready(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        let start = std::time::Instant::now();
        loop {
            if self.read_status()? & SR_WIP == 0 { return Ok(()); }
            if start.elapsed() >= self.poll_timeout { return Err(FlashError::Timeout); }
            std::thread::sleep(self.poll_interval);
        }
    }

    fn erase_block(&mut self, addr: u32) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.write_enable()?;
        let a = addr.to_be_bytes();
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_BE64, a[1], a[2], a[3]])])
            .map_err(Self::spi_err)?;
        self.wait_ready()
    }

    /// Erase the whole device (0xC7). Slowest but universally supported.
    pub fn chip_erase(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.write_enable()?;
        self.spi.transaction(&mut [Operation::Write(&[CMD_CE])]).map_err(Self::spi_err)?;
        self.wait_ready()
    }

    /// Erase every 64 KiB block overlapping `[offset, offset+len)`.
    pub fn erase_range(&mut self, offset: usize, len: usize)
        -> Result<(), FlashError<SPI::Error, RST::Error>>
    {
        if len == 0 { return Ok(()); }
        let first = offset - offset % BLOCK_SIZE;
        let end = offset + len;
        let mut a = first;
        while a < end {
            self.erase_block(a as u32)?;
            a += BLOCK_SIZE;
        }
        Ok(())
    }

    fn page_program(&mut self, addr: u32, data: &[u8])
        -> Result<(), FlashError<SPI::Error, RST::Error>>
    {
        debug_assert!(data.len() <= PAGE_SIZE);
        debug_assert!((addr as usize % PAGE_SIZE) + data.len() <= PAGE_SIZE, "page cross");
        self.write_enable()?;
        let a = addr.to_be_bytes();
        let header = [CMD_PP, a[1], a[2], a[3]];
        let mut ops: Vec<Operation<'_, u8>> = Vec::with_capacity(1 + data.len() / self.max_chunk + 1);
        ops.push(Operation::Write(&header));
        for chunk in data.chunks(self.max_chunk) {
            ops.push(Operation::Write(chunk));
        }
        self.spi.transaction(&mut ops).map_err(Self::spi_err)?;
        self.wait_ready()
    }

    /// Program `data` at `offset`, splitting on 256-byte page boundaries.
    /// `progress(bytes_written)` is called after each page.
    pub fn program(&mut self, offset: usize, data: &[u8], mut progress: impl FnMut(usize))
        -> Result<(), FlashError<SPI::Error, RST::Error>>
    {
        let mut written = 0;
        while written < data.len() {
            let addr = offset + written;
            let page_left = PAGE_SIZE - (addr % PAGE_SIZE);
            let n = page_left.min(data.len() - written);
            self.page_program(addr as u32, &data[written..written + n])?;
            written += n;
            progress(written);
        }
        Ok(())
    }

    /// Read `buf.len()` bytes starting at `offset`, in `max_chunk` units.
    pub fn read(&mut self, offset: usize, buf: &mut [u8])
        -> Result<(), FlashError<SPI::Error, RST::Error>>
    {
        let mut done = 0;
        while done < buf.len() {
            let a = ((offset + done) as u32).to_be_bytes();
            let header = [CMD_READ, a[1], a[2], a[3]];
            let n = self.max_chunk.min(buf.len() - done);
            self.spi
                .transaction(&mut [
                    Operation::Write(&header),
                    Operation::Read(&mut buf[done..done + n]),
                ])
                .map_err(Self::spi_err)?;
            done += n;
        }
        Ok(())
    }

    /// Read back and compare against `expected`. `progress(bytes_verified)` per chunk.
    pub fn verify(&mut self, offset: usize, expected: &[u8], mut progress: impl FnMut(usize))
        -> Result<(), FlashError<SPI::Error, RST::Error>>
    {
        let mut done = 0;
        let mut buf = vec![0u8; self.max_chunk];
        while done < expected.len() {
            let n = self.max_chunk.min(expected.len() - done);
            self.read(offset + done, &mut buf[..n])?;
            for i in 0..n {
                if buf[i] != expected[done + i] {
                    return Err(FlashError::VerifyMismatch {
                        addr: offset + done + i,
                        expected: expected[done + i],
                        got: buf[i],
                    });
                }
            }
            done += n;
            progress(done);
        }
        Ok(())
    }

    /// Hold the FPGA in reset (CRESET low, tri-states the shared bus).
    pub fn acquire_bus(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.reset.acquire().map_err(Self::bus_err)
    }

    /// Release CRESET (Hi-Z) so the FPGA reconfigures from flash.
    pub fn release_bus(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.reset.release().map_err(Self::bus_err)
    }

    /// Full flow: reset → erase covered region → program → (verify) → release.
    /// On any error the caller should still call `release_bus` (see main.rs).
    pub fn flash_bitstream(
        &mut self,
        offset: usize,
        image: &[u8],
        verify: bool,
        flash_size: Option<usize>,
        mut progress: impl FnMut(Progress),
    ) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        if let Some(sz) = flash_size {
            if offset + image.len() > sz {
                return Err(FlashError::TooLarge { need: offset + image.len(), have: sz });
            }
        }
        self.acquire_bus()?;
        progress(Progress::Erasing);
        self.erase_range(offset, image.len())?;
        progress(Progress::Programming(0));
        self.program(offset, image, |w| progress(Progress::Programming(w)))?;
        if verify {
            progress(Progress::Verifying(0));
            self.verify(offset, image, |v| progress(Progress::Verifying(v)))?;
        }
        self.release_bus()?;
        progress(Progress::Done);
        Ok(())
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

    #[test]
    fn wait_ready_polls_until_wip_clears() {
        let flash = FakeFlash::new(1024, [0x20, 0x20, 0x15]);
        flash.set_busy_reads(3); // WIP=1 for 3 RDSR calls, then clears
        let mut f = flasher(flash, FakeBus::new(), 256);
        f.write_enable().unwrap();
        assert_eq!(f.read_status().unwrap() & 0x02, 0x02); // WEL set
        f.wait_ready().unwrap(); // consumes the 3 busy reads, then returns Ok
    }

    #[test]
    fn wait_ready_times_out() {
        let flash = FakeFlash::new(1024, [0x20, 0x20, 0x15]);
        flash.set_busy_reads(u32::MAX); // never clears
        let mut f = Flasher::with_config(
            flash, FakeBus::new(), 256, Duration::ZERO, Duration::ZERO);
        assert!(matches!(f.wait_ready(), Err(FlashError::Timeout)));
    }

    #[test]
    fn erase_range_sets_ff_across_covered_blocks() {
        let size = 256 * 1024; // 4 blocks
        let flash = FakeFlash::new(size, [0x20, 0x20, 0x15]);
        for i in 0..size { flash.preset(i, &[0x00]); } // dirty everything
        let probe = flash.clone();
        let mut f = flasher(flash, FakeBus::new(), 256);

        // A 1-byte image at offset 100 must erase exactly block 0 (0..64K).
        f.erase_range(100, 1).unwrap();
        let mem = probe.mem();
        assert!(mem[0..BLOCK_SIZE].iter().all(|&b| b == 0xFF));      // block 0 erased
        assert!(mem[BLOCK_SIZE..].iter().all(|&b| b == 0x00));        // block 1+ untouched
    }

    #[test]
    fn chip_erase_clears_all() {
        let flash = FakeFlash::new(BLOCK_SIZE, [0x20, 0x20, 0x15]);
        for i in 0..BLOCK_SIZE { flash.preset(i, &[0x00]); }
        let probe = flash.clone();
        let mut f = flasher(flash, FakeBus::new(), 256);
        f.chip_erase().unwrap();
        assert!(probe.mem().iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn program_crossing_page_and_small_chunks() {
        let flash = FakeFlash::new(64 * 1024, [0x20, 0x20, 0x15]);
        let probe = flash.clone();
        // max_chunk = 8 forces multi-op writes within a page program.
        let mut f = flasher(flash, FakeBus::new(), 8);

        // 300 bytes starting at 250 → crosses the 256 boundary (pages 0 and 1).
        let data: Vec<u8> = (0..300).map(|i| (i % 251) as u8).collect();
        f.erase_range(0, 300).unwrap();
        f.program(250, &data, |_| {}).unwrap();

        assert_eq!(&probe.mem()[250..250 + 300], &data[..]);
        // Byte just before the write is still erased.
        assert_eq!(probe.mem()[249], 0xFF);
    }

    #[test]
    fn read_back_and_verify() {
        let flash = FakeFlash::new(64 * 1024, [0x20, 0x20, 0x15]);
        let mut f = flasher(flash, FakeBus::new(), 8); // tiny chunk exercises splitting
        let data: Vec<u8> = (0..500).map(|i| (i * 7 % 256) as u8).collect();
        f.erase_range(0, 500).unwrap();
        f.program(0, &data, |_| {}).unwrap();

        let mut buf = vec![0u8; 500];
        f.read(0, &mut buf).unwrap();
        assert_eq!(buf, data);

        f.verify(0, &data, |_| {}).unwrap();

        // A mismatch is reported with the absolute address.
        let mut wrong = data.clone();
        wrong[123] ^= 0xFF;
        match f.verify(0, &wrong, |_| {}) {
            Err(FlashError::VerifyMismatch { addr, .. }) => assert_eq!(addr, 123),
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn flash_bitstream_end_to_end() {
        let flash = FakeFlash::new(2 * 1024 * 1024, [0x1C, 0x70, 0x15]);
        let probe = flash.clone();
        let reset = FakeBus::new();
        let reset_probe = reset.clone();
        let mut f = flasher(flash, reset, 64);

        let image: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        let mut events = Vec::new();
        f.flash_bitstream(0, &image, true, Some(2 * 1024 * 1024), |p| events.push(p))
            .unwrap();

        assert_eq!(&probe.mem()[..1000], &image[..]);
        assert!(!reset_probe.asserted(), "CRESET must be released at the end");
        assert_eq!(events.first(), Some(&Progress::Erasing));
        assert_eq!(events.last(), Some(&Progress::Done));
    }

    #[test]
    fn flash_bitstream_rejects_oversize() {
        let flash = FakeFlash::new(1024, [0x1C, 0x70, 0x15]);
        let mut f = flasher(flash, FakeBus::new(), 64);
        let image = vec![0u8; 2048];
        assert!(matches!(
            f.flash_bitstream(0, &image, false, Some(1024), |_| {}),
            Err(FlashError::TooLarge { .. })
        ));
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
