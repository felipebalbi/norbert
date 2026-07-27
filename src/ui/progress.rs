//! Restrained multi-phase progress: aligned Unicode bars for erase/program/verify.
//! Inert in Machine mode. Draws to stderr (indicatif's default), leaving stdout
//! (voice) clean.
#![allow(dead_code)] // wired by the write handlers in Phase 5

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use super::Mode;

/// Which phases a run will show, and their lengths.
pub struct ProgressPlan {
    pub erase_blocks: Option<u64>,
    pub program_bytes: Option<u64>,
    pub verify_bytes: Option<u64>,
}

pub struct Progress {
    erase: Option<ProgressBar>,
    program: Option<ProgressBar>,
    verify: Option<ProgressBar>,
    // Held only to keep the shared draw target alive; never read directly.
    #[allow(dead_code)]
    mp: Option<MultiProgress>,
}

fn bytes_bar(mp: &MultiProgress, label: &str, len: u64) -> ProgressBar {
    let pb = mp.add(ProgressBar::new(len));
    pb.set_style(
        ProgressStyle::with_template(
            "  {prefix:<8} [{bar:26}] {bytes:>9}/{total_bytes:<9} {bytes_per_sec:>11}  {eta:>5}",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    pb.set_prefix(label.to_string());
    pb
}

fn blocks_bar(mp: &MultiProgress, label: &str, blocks: u64) -> ProgressBar {
    let pb = mp.add(ProgressBar::new(blocks));
    pb.set_style(
        ProgressStyle::with_template("  {prefix:<8} [{bar:26}] {pos:>4}/{len:<4} blocks")
            .unwrap()
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    pb.set_prefix(label.to_string());
    pb
}

impl Progress {
    /// Build a view for `plan`. In Machine mode everything is inert (no draw).
    pub fn new(mode: Mode, plan: ProgressPlan) -> Progress {
        if mode == Mode::Machine {
            return Progress {
                erase: None,
                program: None,
                verify: None,
                mp: None,
            };
        }
        let mp = MultiProgress::new();
        let erase = plan.erase_blocks.map(|n| blocks_bar(&mp, "erase", n));
        let program = plan.program_bytes.map(|n| bytes_bar(&mp, "program", n));
        let verify = plan.verify_bytes.map(|n| bytes_bar(&mp, "verify", n));
        Progress {
            erase,
            program,
            verify,
            mp: Some(mp),
        }
    }

    pub fn erase_to(&self, blocks: usize) {
        if let Some(b) = &self.erase {
            b.set_position(blocks as u64);
        }
    }
    pub fn program_to(&self, bytes: usize) {
        if let Some(b) = &self.program {
            b.set_position(bytes as u64);
        }
    }
    pub fn verify_to(&self, bytes: usize) {
        if let Some(b) = &self.verify {
            b.set_position(bytes as u64);
        }
    }

    /// Clear all bars so the summary voice line (stdout) prints cleanly.
    pub fn finish(self) {
        for b in [self.erase, self.program, self.verify]
            .into_iter()
            .flatten()
        {
            b.finish_and_clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inert_in_machine_mode() {
        let p = Progress::new(
            Mode::Machine,
            ProgressPlan {
                erase_blocks: Some(3),
                program_bytes: Some(100),
                verify_bytes: None,
            },
        );
        p.erase_to(1);
        p.program_to(50);
        p.verify_to(0);
        p.finish(); // must not panic and must draw nothing
    }
}
