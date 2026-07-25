//! Hardware-agnostic iCE40HX8K-EVB SPI-NOR flashing core.

use core::fmt;
use embedded_hal::spi::{Operation, SpiDevice};
use std::time::Duration;

use crate::sfdp::{
    lookup_fallback, plan_erase, Bfpt, FlashProfile, ParamHeader, ProfileSource, SfdpHeader,
};

// Universal SPI-NOR opcodes (M25P16 / EN25QH16B / W25Q16).
const CMD_RDID: u8 = 0x9F;
const CMD_WREN: u8 = 0x06;
const CMD_WRSR: u8 = 0x01; // write status register (clears block-protect bits)
const CMD_RDSR: u8 = 0x05;
const CMD_READ: u8 = 0x03;
const CMD_PP: u8 = 0x02;
const CMD_CE: u8 = 0xC7; // chip erase
const CMD_SFDP: u8 = 0x5A; // read SFDP parameter space
const CMD_EN4B: u8 = 0xB7; // enter 4-byte addressing mode
const CMD_RSTEN: u8 = 0x66; // enable reset
const CMD_RST: u8 = 0x99; // software reset
const CMD_RELEASE_PD: u8 = 0xAB; // Release from Deep Power-Down
const SR_WIP: u8 = 0x01;
const SR_BP_MASK: u8 = 0x1C; // BP0..BP2 (typical)

/// Append `opcode` + a 3- or 4-byte big-endian address.
fn push_cmd_addr(cmd: &mut Vec<u8>, opcode: u8, addr: u32, addr_bytes: u8) {
    cmd.push(opcode);
    if addr_bytes == 4 {
        cmd.push((addr >> 24) as u8);
    }
    cmd.push((addr >> 16) as u8);
    cmd.push((addr >> 8) as u8);
    cmd.push(addr as u8);
}

/// 256-byte program page.
pub const PAGE_SIZE: usize = 256;
/// 64 KiB erase block.
#[allow(dead_code)] // reference constant; used by tests (erase geometry now comes from FlashProfile)
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

    pub fn jedec(&self) -> [u8; 3] {
        [self.manufacturer, self.mem_type, self.capacity_code]
    }
}

impl fmt::Display for FlashId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "mfr=0x{:02X} type=0x{:02X} cap=0x{:02X}",
            self.manufacturer, self.mem_type, self.capacity_code
        )?;
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
    fn acquire(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn release(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Progress callback events for the high-level flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // full-flow helper; retained + tested, CLI drives the steps directly since Task 23b
pub enum Progress {
    Detecting, // reading SFDP / choosing a flash profile (emitted once SFDP lands, Task 15)
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
    VerifyMismatch {
        addr: usize,
        expected: u8,
        got: u8,
    },
    #[allow(dead_code)]
    // constructed by flash_bitstream (tested); CLI uses erase_range's own guard
    TooLarge {
        need: usize,
        have: usize,
    },
    /// RDID read nothing — no chip on the bus.
    NoFlash,
    /// A chip responded but has no SFDP and no fallback-table entry.
    UnsupportedChip {
        jedec: [u8; 3],
    },
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
    profile: Option<FlashProfile>,
}

impl<SPI, RST> Flasher<SPI, RST>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    /// Sensible defaults: 256-byte SPI chunks, 2 ms poll interval, 60 s timeout.
    #[allow(dead_code)] // library convenience (CLI uses with_config)
    pub fn new(spi: SPI, reset: RST) -> Self {
        Self::with_config(
            spi,
            reset,
            PAGE_SIZE,
            Duration::from_millis(2),
            Duration::from_secs(60),
        )
    }

    /// `max_chunk` bounds bytes per USB SPI op; all writes/reads are split to it
    /// *within* a single CS transaction, so any value >= 4 is correct.
    pub fn with_config(
        spi: SPI,
        reset: RST,
        max_chunk: usize,
        poll_interval: Duration,
        poll_timeout: Duration,
    ) -> Self {
        assert!(max_chunk >= 4, "max_chunk must be >= 4");
        Self {
            spi,
            reset,
            max_chunk,
            poll_interval,
            poll_timeout,
            profile: None,
        }
    }

    fn spi_err(e: SPI::Error) -> FlashError<SPI::Error, RST::Error> {
        FlashError::Spi(e)
    }
    fn bus_err(e: RST::Error) -> FlashError<SPI::Error, RST::Error> {
        FlashError::Bus(e)
    }

    /// Read the JEDEC ID (0x9F).
    pub fn read_id(&mut self) -> Result<FlashId, FlashError<SPI::Error, RST::Error>> {
        let mut id = [0u8; 3];
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_RDID]), Operation::Read(&mut id)])
            .map_err(Self::spi_err)?;
        Ok(FlashId {
            manufacturer: id[0],
            mem_type: id[1],
            capacity_code: id[2],
        })
    }

    /// Read `buf.len()` bytes from SFDP space at `addr`
    /// (0x5A + 24-bit addr + 8 dummy cycles).
    pub fn read_sfdp(
        &mut self,
        addr: u32,
        buf: &mut [u8],
    ) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        let a = addr.to_be_bytes();
        let header = [CMD_SFDP, a[1], a[2], a[3], 0x00]; // trailing byte = 8 dummy cycles
        self.spi
            .transaction(&mut [Operation::Write(&header), Operation::Read(buf)])
            .map_err(Self::spi_err)
    }

    fn write_enable(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_WREN])])
            .map_err(Self::spi_err)
    }

    /// Switch the device to 32-bit addressing (needed for >16 MB parts).
    fn enter_4byte(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_EN4B])])
            .map_err(Self::spi_err)
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
            if self.read_status()? & SR_WIP == 0 {
                return Ok(());
            }
            if start.elapsed() >= self.poll_timeout {
                return Err(FlashError::Timeout);
            }
            std::thread::sleep(self.poll_interval);
        }
    }

    /// Active flash profile, or `None` until `detect_profile` succeeds.
    pub fn profile(&self) -> Option<&FlashProfile> {
        self.profile.as_ref()
    }
    pub fn set_profile(&mut self, profile: FlashProfile) {
        self.profile = Some(profile);
    }

    fn require_profile(&self) -> Result<&FlashProfile, FlashError<SPI::Error, RST::Error>> {
        self.profile.as_ref().ok_or(FlashError::NotDetected)
    }

    /// Resolve how to talk to the flash and store the profile in `self`:
    /// RDID (absent → `NoFlash`) → SFDP → JEDEC fallback table → else
    /// `UnsupportedChip`. **Never guesses.** Requires the bus (call while acquired).
    pub fn detect_profile(&mut self) -> Result<FlashProfile, FlashError<SPI::Error, RST::Error>> {
        let id = self.read_id()?;
        if !id.is_present() {
            return Err(FlashError::NoFlash);
        }
        // 1) The chip describes itself.
        if let Some(profile) = self.try_sfdp_profile(id)? {
            return self.adopt_profile(profile);
        }
        // 2) A known no-SFDP part.
        if let Some(profile) = lookup_fallback(id.jedec()) {
            return self.adopt_profile(profile);
        }
        // 3) Present but unknown → don't guess.
        Err(FlashError::UnsupportedChip { jedec: id.jedec() })
    }

    /// Store `profile` in `self`, first putting >16 MB parts into 4-byte
    /// addressing so subsequent read/erase/program headers match.
    fn adopt_profile(
        &mut self,
        profile: FlashProfile,
    ) -> Result<FlashProfile, FlashError<SPI::Error, RST::Error>> {
        if profile.address_bytes == 4 {
            self.enter_4byte()?;
        }
        self.set_profile(profile.clone());
        Ok(profile)
    }

    /// Build a profile from SFDP; `Ok(None)` if the chip has no usable SFDP.
    fn try_sfdp_profile(
        &mut self,
        id: FlashId,
    ) -> Result<Option<FlashProfile>, FlashError<SPI::Error, RST::Error>> {
        let mut header = [0u8; 8];
        self.read_sfdp(0, &mut header)?;
        let Some(h) = SfdpHeader::parse(&header) else {
            return Ok(None);
        };

        let mut bfpt_ph = None;
        for i in 0..h.param_header_count() {
            let mut ph = [0u8; 8];
            self.read_sfdp(8 + (i as u32) * 8, &mut ph)?;
            if let Some(p) = ParamHeader::parse(&ph) {
                if p.id == ParamHeader::BFPT_ID {
                    bfpt_ph = Some(p);
                    break;
                }
            }
        }
        let Some(p) = bfpt_ph else {
            return Ok(None);
        };

        let mut bytes = vec![0u8; p.length_dwords as usize * 4];
        self.read_sfdp(p.table_pointer, &mut bytes)?;
        let bfpt = Bfpt::parse(&bytes);
        if bfpt.erase_types.is_empty() {
            return Ok(None); // SFDP present but unusable → fall through to the table
        }
        Ok(Some(FlashProfile {
            page_size: bfpt.page_size,
            address_bytes: bfpt.address_bytes,
            capacity: bfpt.capacity.or(id.capacity_bytes()),
            erase_types: bfpt.erase_types,
            source: ProfileSource::Sfdp,
        }))
    }

    /// Erase one block/sector at `addr` using erase `opcode`.
    fn erase_op(
        &mut self,
        addr: u32,
        opcode: u8,
    ) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        let ab = self.require_profile()?.address_bytes;
        self.write_enable()?;
        let mut header = Vec::with_capacity(1 + ab as usize);
        push_cmd_addr(&mut header, opcode, addr, ab);
        self.spi
            .transaction(&mut [Operation::Write(&header)])
            .map_err(Self::spi_err)?;
        self.wait_ready()
    }

    /// Erase the whole device (0xC7). Slowest but universally supported.
    pub fn chip_erase(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.write_enable()?;
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_CE])])
            .map_err(Self::spi_err)?;
        self.wait_ready()
    }

    /// Clear status-register block-protection bits (WREN + WRSR 0x00).
    pub fn unprotect(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.write_enable()?;
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_WRSR, 0x00])])
            .map_err(Self::spi_err)?;
        self.wait_ready()
    }

    /// True if any block-protect bit is set.
    pub fn is_protected(&mut self) -> Result<bool, FlashError<SPI::Error, RST::Error>> {
        Ok(self.read_status()? & SR_BP_MASK != 0)
    }

    /// Set block-protection (WREN + WRSR with BP bits set).
    pub fn protect(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.write_enable()?;
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_WRSR, SR_BP_MASK])])
            .map_err(Self::spi_err)?;
        self.wait_ready()
    }

    /// Flash software reset: 0x66 (enable) then 0x99 (reset).
    pub fn reset_flash(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_RSTEN])])
            .map_err(Self::spi_err)?;
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_RST])])
            .map_err(Self::spi_err)
    }

    /// Erase every block overlapping `[offset, offset+len)`, choosing erase
    /// granularities from the detected profile (`sfdp::plan_erase`).
    pub fn erase_range(
        &mut self,
        offset: usize,
        len: usize,
    ) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        let plan = plan_erase(self.require_profile()?, offset, len);
        for (addr, opcode) in plan {
            self.erase_op(addr as u32, opcode)?;
        }
        Ok(())
    }

    fn page_program(
        &mut self,
        addr: u32,
        data: &[u8],
    ) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        let ab = self.require_profile()?.address_bytes;
        self.write_enable()?;
        let mut header = Vec::with_capacity(1 + ab as usize);
        push_cmd_addr(&mut header, CMD_PP, addr, ab);
        let mut ops: Vec<Operation<'_, u8>> =
            Vec::with_capacity(1 + data.len() / self.max_chunk + 1);
        ops.push(Operation::Write(&header));
        for chunk in data.chunks(self.max_chunk) {
            ops.push(Operation::Write(chunk));
        }
        self.spi.transaction(&mut ops).map_err(Self::spi_err)?;
        self.wait_ready()
    }

    /// Program `data` at `offset`, splitting on the profile's page boundaries.
    /// `progress(bytes_written)` is called after each page.
    pub fn program(
        &mut self,
        offset: usize,
        data: &[u8],
        mut progress: impl FnMut(usize),
    ) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        let page = self.require_profile()?.page_size;
        let mut written = 0;
        while written < data.len() {
            let addr = offset + written;
            let page_left = page - (addr % page);
            let n = page_left.min(data.len() - written);
            self.page_program(addr as u32, &data[written..written + n])?;
            written += n;
            progress(written);
        }
        Ok(())
    }

    /// Read `buf.len()` bytes starting at `offset`, in `max_chunk` units.
    pub fn read(
        &mut self,
        offset: usize,
        buf: &mut [u8],
    ) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        let ab = self.require_profile()?.address_bytes;
        let mut done = 0;
        while done < buf.len() {
            let mut header = Vec::with_capacity(1 + ab as usize);
            push_cmd_addr(&mut header, CMD_READ, (offset + done) as u32, ab);
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
    pub fn verify(
        &mut self,
        offset: usize,
        expected: &[u8],
        mut progress: impl FnMut(usize),
    ) -> Result<(), FlashError<SPI::Error, RST::Error>> {
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

    /// Release the flash from Deep Power-Down (0xAB), then wait tRES1.
    ///
    /// An iCE40 (and similar) puts its configuration flash into deep power-down
    /// after loading its bitstream; in that state the flash ignores every command
    /// except 0xAB. This is a no-op on a flash that is already awake.
    pub fn wake(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_RELEASE_PD])])
            .map_err(Self::spi_err)?;
        // tRES1 is ~3 us on a W25Q; a USB round-trip already covers it, but be explicit.
        std::thread::sleep(Duration::from_micros(50));
        Ok(())
    }

    /// Take the shared bus (hold any other master off) and wake the flash from
    /// Deep Power-Down, so a flash the FPGA put to sleep after config responds.
    pub fn acquire_bus(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.reset.acquire().map_err(Self::bus_err)?;
        self.wake()
    }

    /// Release CRESET (Hi-Z) so the FPGA reconfigures from flash.
    pub fn release_bus(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.reset.release().map_err(Self::bus_err)
    }

    /// Full flow: acquire bus → (detect) → erase covered region → program → (verify) → release.
    /// On any error the caller should still call `release_bus` (see main.rs).
    #[allow(dead_code)] // retained + tested; CLI drives detect/erase/program/verify directly since Task 23b
    pub fn flash_bitstream(
        &mut self,
        offset: usize,
        image: &[u8],
        detect: bool,
        unprotect: bool,
        verify: bool,
        flash_size: Option<usize>,
        mut progress: impl FnMut(Progress),
    ) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.acquire_bus()?;
        if detect {
            progress(Progress::Detecting);
            self.detect_profile()?;
        }
        let size = self.profile().and_then(|p| p.capacity).or(flash_size);
        if let Some(sz) = size {
            if offset + image.len() > sz {
                let _ = self.release_bus();
                return Err(FlashError::TooLarge {
                    need: offset + image.len(),
                    have: sz,
                });
            }
        }
        if unprotect {
            self.unprotect()?;
        }
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
        let id = FlashId {
            manufacturer: 0x20,
            mem_type: 0x20,
            capacity_code: 0x15,
        };
        assert_eq!(id.capacity_bytes(), Some(2 * 1024 * 1024)); // 16 Mbit = 2 MiB
        let bad = FlashId {
            manufacturer: 0,
            mem_type: 0,
            capacity_code: 0x00,
        };
        assert_eq!(bad.capacity_bytes(), None);
    }

    #[test]
    fn read_id_returns_jedec() {
        let flash = FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]);
        let mut f = flasher(flash, FakeBus::new(), 256);
        let id = f.read_id().unwrap();
        assert_eq!(
            id,
            FlashId {
                manufacturer: 0x20,
                mem_type: 0x20,
                capacity_code: 0x15
            }
        );
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
        let mut f =
            Flasher::with_config(flash, FakeBus::new(), 256, Duration::ZERO, Duration::ZERO);
        assert!(matches!(f.wait_ready(), Err(FlashError::Timeout)));
    }

    #[test]
    fn erase_range_sets_ff_across_covered_blocks() {
        let size = 256 * 1024; // 4 blocks
        let flash = FakeFlash::new(size, [0x20, 0x20, 0x15]);
        for i in 0..size {
            flash.preset(i, &[0x00]);
        } // dirty everything
        let probe = flash.clone();
        let mut f = flasher(flash, FakeBus::new(), 256);

        // A 1-byte image at offset 100 must erase exactly block 0 (0..64K).
        f.erase_range(100, 1).unwrap();
        let mem = probe.mem();
        assert!(mem[0..BLOCK_SIZE].iter().all(|&b| b == 0xFF)); // block 0 erased
        assert!(mem[BLOCK_SIZE..].iter().all(|&b| b == 0x00)); // block 1+ untouched
    }

    #[test]
    fn chip_erase_clears_all() {
        let flash = FakeFlash::new(BLOCK_SIZE, [0x20, 0x20, 0x15]);
        for i in 0..BLOCK_SIZE {
            flash.preset(i, &[0x00]);
        }
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
        f.flash_bitstream(0, &image, false, false, true, Some(2 * 1024 * 1024), |p| {
            events.push(p)
        })
        .unwrap();

        assert_eq!(&probe.mem()[..1000], &image[..]);
        assert!(
            !reset_probe.asserted(),
            "CRESET must be released at the end"
        );
        assert_eq!(events.first(), Some(&Progress::Erasing));
        assert_eq!(events.last(), Some(&Progress::Done));
    }

    #[test]
    fn flash_bitstream_rejects_oversize() {
        let flash = FakeFlash::new(1024, [0x20, 0x20, 0x15]);
        let mut f = flasher(flash, FakeBus::new(), 64);
        f.set_profile(crate::sfdp::FlashProfile {
            page_size: 256,
            address_bytes: 3,
            capacity: Some(1024),
            erase_types: vec![crate::sfdp::EraseType {
                size: 64 * 1024,
                opcode: 0xD8,
            }],
            source: crate::sfdp::ProfileSource::Table,
        });
        let image = vec![0u8; 2048];
        assert!(matches!(
            f.flash_bitstream(0, &image, false, false, false, None, |_| {}),
            Err(FlashError::TooLarge { .. })
        ));
    }

    #[test]
    fn erase_uses_profile_small_sector_at_tail() {
        use crate::sfdp::{EraseType, FlashProfile, ProfileSource};
        let size = 256 * 1024;
        let flash = FakeFlash::new(size, [0xEF, 0x40, 0x15]);
        for i in 0..size {
            flash.preset(i, &[0x00]);
        }
        let probe = flash.clone();
        let mut f = flasher(flash, FakeBus::new(), 256);
        f.set_profile(FlashProfile {
            page_size: 256,
            address_bytes: 3,
            capacity: Some(size),
            erase_types: vec![
                EraseType {
                    size: 64 * 1024,
                    opcode: 0xD8,
                },
                EraseType {
                    size: 4 * 1024,
                    opcode: 0x20,
                },
            ],
            source: ProfileSource::Sfdp,
        });
        f.erase_range(0, 131_072 + 100).unwrap();
        let mem = probe.mem();
        assert!(mem[0..131_072 + 4096].iter().all(|&b| b == 0xFF)); // erased through tail sector
        assert!(mem[131_072 + 4096..].iter().all(|&b| b == 0x00)); // nothing beyond
    }

    #[test]
    fn erase_without_profile_is_not_detected() {
        let flash = FakeFlash::new(1024, [0xEF, 0x40, 0x15]);
        let mut f = Flasher::with_config(
            flash,
            FakeBus::new(),
            256,
            Duration::ZERO,
            Duration::from_secs(1),
        );
        assert!(matches!(f.erase_range(0, 10), Err(FlashError::NotDetected)));
    }

    #[test]
    fn read_sfdp_returns_blob() {
        let flash = FakeFlash::new(1024, [0xEF, 0x40, 0x15]);
        flash.set_sfdp(&[0x53, 0x46, 0x44, 0x50, 0x06, 0x01, 0x00, 0xFF]);
        let mut f = flasher(flash, FakeBus::new(), 256);
        let mut hdr = [0u8; 8];
        f.read_sfdp(0, &mut hdr).unwrap();
        assert_eq!(&hdr[0..4], b"SFDP");
    }

    // Full SFDP image: header@0, BFPT param header@8, BFPT@0x10.
    fn sfdp_blob() -> Vec<u8> {
        let mut v = vec![0xFFu8; 0x10];
        v[0..8].copy_from_slice(&[0x53, 0x46, 0x44, 0x50, 0x06, 0x01, 0x00, 0xFF]); // "SFDP" rev1.6 nph=0
        v[8..16].copy_from_slice(&[0x00, 0x01, 0x01, 0x0B, 0x10, 0x00, 0x00, 0xFF]); // BFPT id0xFF00 len11 ptp0x10
        let mut b = vec![0u8; 11 * 4];
        b[0..4].copy_from_slice(&[0xE5, 0x20, 0x00, 0x00]);
        b[4..8].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0x00]);
        b[28..32].copy_from_slice(&[0x0C, 0x20, 0x0F, 0x52]);
        b[32..36].copy_from_slice(&[0x10, 0xD8, 0x00, 0x00]);
        b[40..44].copy_from_slice(&[0x80, 0x00, 0x00, 0x00]);
        v.extend_from_slice(&b);
        v
    }

    #[test]
    fn detect_via_sfdp_builds_profile() {
        use crate::sfdp::ProfileSource;
        let flash = FakeFlash::new(2 * 1024 * 1024, [0xEF, 0x40, 0x15]);
        flash.set_sfdp(&sfdp_blob());
        let mut f = flasher(flash, FakeBus::new(), 256);
        let p = f.detect_profile().unwrap();
        assert_eq!(p.source, ProfileSource::Sfdp);
        assert_eq!(p.page_size, 256);
        assert_eq!(p.capacity, Some(2 * 1024 * 1024));
        assert_eq!(p.erase_types.len(), 3);
        assert_eq!(p.erase_types[0].size, 64 * 1024); // largest first
    }

    #[test]
    fn detect_uses_fallback_table_for_m25p16() {
        use crate::sfdp::{EraseType, ProfileSource};
        let flash = FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]); // M25P16: no SFDP, in table
        let mut f = flasher(flash, FakeBus::new(), 256);
        let p = f.detect_profile().unwrap();
        assert_eq!(p.source, ProfileSource::Table);
        assert_eq!(p.capacity, Some(2 * 1024 * 1024));
        assert_eq!(
            p.erase_types,
            vec![EraseType {
                size: 64 * 1024,
                opcode: 0xD8
            }]
        );
    }

    #[test]
    fn detect_unsupported_chip_errs() {
        // A chip responds, but no SFDP and not in the table.
        let flash = FakeFlash::new(1024, [0xAB, 0xCD, 0xEF]);
        let mut f = flasher(flash, FakeBus::new(), 256);
        assert!(matches!(
            f.detect_profile(),
            Err(FlashError::UnsupportedChip { .. })
        ));
    }

    #[test]
    fn detect_no_flash_errs() {
        // Idle/floating bus: RDID reads all 0xFF.
        let flash = FakeFlash::new(1024, [0xFF, 0xFF, 0xFF]);
        let mut f = flasher(flash, FakeBus::new(), 256);
        assert!(matches!(f.detect_profile(), Err(FlashError::NoFlash)));
    }

    #[test]
    fn flash_bitstream_detects_first() {
        let flash = FakeFlash::new(2 * 1024 * 1024, [0xEF, 0x40, 0x15]);
        flash.set_sfdp(&sfdp_blob());
        let probe = flash.clone();
        let mut f = flasher(flash, FakeBus::new(), 256);
        let image: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        let mut events = Vec::new();
        f.flash_bitstream(0, &image, true, false, true, None, |p| events.push(p))
            .unwrap();
        assert_eq!(events.first(), Some(&Progress::Detecting));
        assert_eq!(&probe.mem()[..1000], &image[..]);
        assert_eq!(
            f.profile().unwrap().source,
            crate::sfdp::ProfileSource::Sfdp
        );
    }

    #[test]
    fn four_byte_addressing_roundtrips() {
        use crate::sfdp::{EraseType, FlashProfile, ProfileSource};
        let flash = FakeFlash::new(1024 * 1024, [0xEF, 0x40, 0x19]); // pretend 256 Mbit part
        let probe = flash.clone();
        let mut f = flasher(flash, FakeBus::new(), 256);
        f.set_profile(FlashProfile {
            page_size: 256,
            address_bytes: 4,
            capacity: Some(32 * 1024 * 1024),
            erase_types: vec![EraseType {
                size: 64 * 1024,
                opcode: 0xD8,
            }],
            source: ProfileSource::Sfdp,
        });
        f.enter_4byte().unwrap(); // put the fake into 4-byte mode
        let addr = 0x0002_0000; // emitted as 4 bytes because the profile says so
        let data: Vec<u8> = (0..600).map(|i| (i % 256) as u8).collect();
        f.erase_range(addr, data.len()).unwrap();
        f.program(addr, &data, |_| {}).unwrap();
        let mut buf = vec![0u8; data.len()];
        f.read(addr, &mut buf).unwrap();
        assert_eq!(buf, data);
        assert_eq!(&probe.mem()[addr..addr + data.len()], &data[..]);
    }

    #[test]
    fn unprotect_enables_programming() {
        let flash = FakeFlash::new(1024, [0xEF, 0x40, 0x15]);
        flash.set_protected(true);
        let probe = flash.clone();
        let mut f = flasher(flash, FakeBus::new(), 256);
        f.program(0, &[1, 2, 3, 4], |_| {}).unwrap();
        assert_eq!(&probe.mem()[0..4], &[0xFF, 0xFF, 0xFF, 0xFF]); // blocked while protected
        f.unprotect().unwrap();
        f.program(0, &[1, 2, 3, 4], |_| {}).unwrap();
        assert_eq!(&probe.mem()[0..4], &[1, 2, 3, 4]); // now written
    }

    #[test]
    fn protect_then_unprotect_roundtrip() {
        let flash = FakeFlash::new(1024, [0xEF, 0x40, 0x15]);
        let mut f = flasher(flash, FakeBus::new(), 256);
        assert!(!f.is_protected().unwrap());
        f.protect().unwrap();
        assert!(f.is_protected().unwrap());
        f.unprotect().unwrap();
        assert!(!f.is_protected().unwrap());
    }

    #[test]
    fn soft_reset_runs() {
        let flash = FakeFlash::new(1024, [0xEF, 0x40, 0x15]);
        let mut f = flasher(flash, FakeBus::new(), 256);
        f.reset_flash().unwrap(); // 0x66 then 0x99
    }

    #[test]
    fn wake_releases_power_down() {
        // An iCE40 leaves its config flash in Deep Power-Down after booting.
        let flash = FakeFlash::new(2 * 1024 * 1024, [0xEF, 0x40, 0x18]);
        flash.set_powered_down(true);
        let mut f = flasher(flash, FakeBus::new(), 256);
        // Asleep: RDID reads 0xFF -> not present.
        assert!(!f.read_id().unwrap().is_present());
        // Wake it, then RDID works.
        f.wake().unwrap();
        assert_eq!(
            f.read_id().unwrap(),
            FlashId {
                manufacturer: 0xEF,
                mem_type: 0x40,
                capacity_code: 0x18
            }
        );
    }

    #[test]
    fn acquire_bus_wakes_the_flash() {
        let flash = FakeFlash::new(2 * 1024 * 1024, [0xEF, 0x40, 0x18]);
        flash.set_powered_down(true);
        let mut f = flasher(flash, FakeBus::new(), 256);
        f.acquire_bus().unwrap(); // must hold the bus AND wake the flash
        assert!(f.read_id().unwrap().is_present());
    }

    #[derive(Debug)]
    pub struct FakeErr;
    impl SpiErrorTrait for FakeErr {
        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    struct FakeState {
        mem: Vec<u8>,
        id: [u8; 3],
        wel: bool,
        busy_reads: u32, // # of RDSR calls that report WIP=1 before clearing
        sfdp: Vec<u8>,
        mode4b: bool,       // true once EN4B (0xB7) seen → 32-bit addresses
        protected: bool,    // true once BP bits set → PAGE PROGRAM silently no-ops
        powered_down: bool, // true once 0xB9 seen → ignores all but 0xAB (models the iCE40)
    }

    /// Shareable behavioral SPI-NOR. Clone to inspect memory after moving one
    /// clone into a `Flasher`.
    #[derive(Clone)]
    struct FakeFlash(Rc<RefCell<FakeState>>);

    impl FakeFlash {
        fn new(size: usize, id: [u8; 3]) -> Self {
            FakeFlash(Rc::new(RefCell::new(FakeState {
                mem: vec![0xFF; size],
                id,
                wel: false,
                busy_reads: 0,
                sfdp: Vec::new(),
                mode4b: false,
                protected: false,
                powered_down: false,
            })))
        }
        fn set_busy_reads(&self, n: u32) {
            self.0.borrow_mut().busy_reads = n;
        }
        fn set_sfdp(&self, blob: &[u8]) {
            self.0.borrow_mut().sfdp = blob.to_vec();
        }
        fn set_protected(&self, p: bool) {
            self.0.borrow_mut().protected = p;
        }
        fn set_powered_down(&self, p: bool) {
            self.0.borrow_mut().powered_down = p;
        }
        fn mem(&self) -> Vec<u8> {
            self.0.borrow().mem.clone()
        }
        fn preset(&self, addr: usize, bytes: &[u8]) {
            self.0.borrow_mut().mem[addr..addr + bytes.len()].copy_from_slice(bytes);
        }
    }

    impl ErrorType for FakeFlash {
        type Error = FakeErr;
    }

    impl SpiDevice for FakeFlash {
        fn transaction(&mut self, ops: &mut [Operation<'_, u8>]) -> Result<(), FakeErr> {
            // Flatten MOSI bytes; collect MISO destinations in order.
            let mut mosi: Vec<u8> = Vec::new();
            let mut reads: Vec<&mut [u8]> = Vec::new();
            for op in ops.iter_mut() {
                match op {
                    Operation::Write(w) => mosi.extend_from_slice(w),
                    Operation::Read(r) => reads.push(r),
                    Operation::Transfer(r, w) => {
                        mosi.extend_from_slice(w);
                        reads.push(r);
                    }
                    Operation::TransferInPlace(b) => {
                        let c = b.to_vec();
                        mosi.extend_from_slice(&c);
                        reads.push(b);
                    }
                    Operation::DelayNs(_) => {}
                }
            }
            if mosi.is_empty() {
                return Ok(());
            }

            let mut st = self.0.borrow_mut();
            let mut resp: Vec<u8> = Vec::new();
            let ab = if st.mode4b { 4 } else { 3 };
            let addr_at = |m: &[u8]| -> usize {
                let mut a = 0usize;
                for i in 0..ab {
                    a = (a << 8) | m[1 + i] as usize;
                }
                a
            };
            match mosi[0] {
                CMD_RELEASE_PD => st.powered_down = false, // 0xAB wakes it
                0xB9 => st.powered_down = true, // 0xB9 = Deep Power-Down (models the FPGA)
                _ if st.powered_down => {}      // asleep: ignore every other command
                CMD_RDID => resp.extend_from_slice(&st.id),
                CMD_WREN => st.wel = true,
                CMD_EN4B => st.mode4b = true,
                CMD_RDSR => {
                    let wip = if st.busy_reads > 0 {
                        st.busy_reads -= 1;
                        SR_WIP
                    } else {
                        0
                    };
                    let wel = if st.wel { 0x02 } else { 0 };
                    let bp = if st.protected { 0x1C } else { 0 };
                    resp.push(wip | wel | bp);
                }
                CMD_READ => {
                    let a = addr_at(&mosi);
                    let total: usize = reads.iter().map(|r| r.len()).sum();
                    for i in 0..total {
                        resp.push(*st.mem.get(a + i).unwrap_or(&0xFF));
                    }
                }
                CMD_SFDP => {
                    let a = u32::from_be_bytes([0, mosi[1], mosi[2], mosi[3]]) as usize;
                    let total: usize = reads.iter().map(|r| r.len()).sum();
                    for i in 0..total {
                        resp.push(*st.sfdp.get(a + i).unwrap_or(&0xFF));
                    }
                }
                CMD_WRSR => {
                    if st.wel {
                        let sr = mosi.get(1).copied().unwrap_or(0);
                        st.protected = sr & 0x1C != 0; // any BP bit set
                    }
                    st.wel = false;
                }
                CMD_PP => {
                    let a = addr_at(&mosi);
                    if st.wel && !st.protected {
                        for (i, b) in mosi[1 + ab..].iter().enumerate() {
                            let page = a & !(PAGE_SIZE - 1);
                            let off = (a + i - page) % PAGE_SIZE; // wraps within page
                            st.mem[page + off] &= *b; // NOR: program clears bits
                        }
                    }
                    st.wel = false;
                }
                CMD_CE => {
                    if st.wel {
                        st.mem.iter_mut().for_each(|b| *b = 0xFF);
                    }
                    st.wel = false;
                }
                op @ (0x20 | 0x52 | 0xD8) => {
                    let size = match op {
                        0x20 => 4 * 1024,
                        0x52 => 32 * 1024,
                        _ => 64 * 1024,
                    };
                    let a = addr_at(&mosi);
                    if st.wel {
                        let base = a & !(size - 1);
                        for b in &mut st.mem[base..base + size] {
                            *b = 0xFF;
                        }
                    }
                    st.wel = false;
                }
                _ => {}
            }
            let mut ri = 0;
            for r in reads.iter_mut() {
                for b in r.iter_mut() {
                    *b = *resp.get(ri).unwrap_or(&0xFF);
                    ri += 1;
                }
            }
            Ok(())
        }
    }

    /// Shareable reset line; `true` = asserted (FPGA held in reset).
    #[derive(Clone)]
    struct FakeBus(Rc<RefCell<bool>>);
    impl FakeBus {
        fn new() -> Self {
            FakeBus(Rc::new(RefCell::new(false)))
        }
        fn asserted(&self) -> bool {
            *self.0.borrow()
        }
    }
    impl BusAccess for FakeBus {
        type Error = Infallible;
        fn acquire(&mut self) -> Result<(), Infallible> {
            *self.0.borrow_mut() = true;
            Ok(())
        }
        fn release(&mut self) -> Result<(), Infallible> {
            *self.0.borrow_mut() = false;
            Ok(())
        }
    }

    /// Fast Flasher over the fakes (zero poll interval so tests don't sleep),
    /// pre-loaded with a default profile so geometry ops resolve.
    fn flasher(flash: FakeFlash, bus: FakeBus, max_chunk: usize) -> Flasher<FakeFlash, FakeBus> {
        let mut f = Flasher::with_config(
            flash,
            bus,
            max_chunk,
            Duration::ZERO,
            Duration::from_secs(1),
        );
        f.set_profile(crate::sfdp::FlashProfile {
            page_size: 256,
            address_bytes: 3,
            capacity: Some(2 * 1024 * 1024),
            erase_types: vec![crate::sfdp::EraseType {
                size: 64 * 1024,
                opcode: 0xD8,
            }],
            source: crate::sfdp::ProfileSource::Table,
        });
        f
    }
}
