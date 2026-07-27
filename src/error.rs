//! The single application-level error. `flash::FlashError` stays generic in the
//! core; this non-generic type is what handlers return and `ui` renders.
#![allow(dead_code)] // wired up in Phase 5; the lint is added there too

use crate::flash::FlashError;
use core::fmt;

#[derive(Debug)]
pub enum NorbertError {
    NoFlash,
    Unsupported([u8; 3]),
    VerifyMismatch { addr: usize },
    Protected,
    Timeout,
    NotDetected,
    Cancelled,
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
