//! The single application-level error. `flash::FlashError` stays generic in the
//! core; this non-generic type is what handlers return and `ui` renders.

use crate::flash::FlashError;
use core::fmt;

#[derive(Debug)]
/// The single application-level error handlers return and [`crate::ui`] renders.
///
/// The generic [`FlashError`] from the core is collapsed into these non-generic
/// variants via [`From`], so the rest of the application never carries the SPI
/// or bus-access error type parameters.
pub enum NorbertError {
    /// No SPI-NOR flash responded to RDID (an idle/floating bus reads all
    /// `0x00`/`0xFF`).
    NoFlash,
    /// A chip responded but has neither SFDP nor a fallback-table entry; carries
    /// its raw 3-byte JEDEC ID.
    Unsupported([u8; 3]),
    /// Read-back did not match the expected image at byte `addr`.
    VerifyMismatch { addr: usize },
    /// A write was refused because status-register block-protection is enabled.
    Protected,
    /// The flash stopped answering within the poll timeout.
    Timeout,
    /// A geometry operation was attempted before `detect` established a profile.
    NotDetected,
    /// The operation was interrupted (Ctrl-C); maps to exit code 130.
    Cancelled,
    /// Any other error (transport faults, I/O, argument validation), preserved
    /// via [`anyhow`].
    Other(anyhow::Error),
}

impl<S: fmt::Debug, R: fmt::Debug> From<FlashError<S, R>> for NorbertError {
    fn from(e: FlashError<S, R>) -> Self {
        match e {
            FlashError::NoFlash => NorbertError::NoFlash,
            FlashError::UnsupportedChip { jedec } => NorbertError::Unsupported(jedec),
            FlashError::VerifyMismatch { addr, .. } => NorbertError::VerifyMismatch { addr },
            FlashError::Timeout => NorbertError::Timeout,
            FlashError::NotDetected => NorbertError::NotDetected,
            // Transport faults keep their fact via FlashError's Display. Matched
            // explicitly (not a catch-all) so a future FlashError variant is a
            // compile error here, not a silent mis-map.
            e @ (FlashError::Spi(_) | FlashError::Bus(_)) => {
                NorbertError::Other(anyhow::anyhow!("{e}"))
            }
        }
    }
}

impl From<anyhow::Error> for NorbertError {
    fn from(e: anyhow::Error) -> Self {
        NorbertError::Other(e)
    }
}

impl From<std::io::Error> for NorbertError {
    fn from(e: std::io::Error) -> Self {
        NorbertError::Other(e.into())
    }
}

impl fmt::Display for NorbertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NorbertError::NoFlash => write!(f, "no SPI-NOR flash detected"),
            NorbertError::Unsupported(j) => write!(f, "unsupported flash {j:02X?}"),
            NorbertError::VerifyMismatch { addr } => {
                write!(f, "verify mismatch at 0x{addr:06X}")
            }
            NorbertError::Protected => write!(f, "write protection enabled"),
            NorbertError::Timeout => write!(f, "timed out waiting for the flash"),
            NorbertError::NotDetected => write!(f, "flash geometry unknown; run detect first"),
            NorbertError::Cancelled => write!(f, "cancelled"),
            NorbertError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for NorbertError {}
