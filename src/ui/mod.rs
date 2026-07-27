//! Norbert's presentation layer: the only module that writes to the terminal.
//! Handlers gather data and sequence these methods; personality text comes from
//! `voice`; this module owns layout and the Human/Machine decision.
#![allow(dead_code)] // fully wired in Phase 5, where the print lint is also added

pub mod progress;

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use crate::error::NorbertError;
use crate::voice;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Human,
    Machine,
}

/// One labeled field. `human`/`machine` differ where units do (e.g. capacity).
pub struct Row {
    pub key: &'static str,
    pub label: &'static str,
    pub human: String,
    pub machine: String,
}

impl Row {
    /// Same text in both modes (chip name, source, …).
    pub fn new(key: &'static str, label: &'static str, value: impl Into<String>) -> Row {
        let v = value.into();
        Row {
            key,
            label,
            human: v.clone(),
            machine: v,
        }
    }
    /// Different text per mode (Human "16384 KiB" vs Machine "16777216").
    pub fn split(
        key: &'static str,
        label: &'static str,
        human: impl Into<String>,
        machine: impl Into<String>,
    ) -> Row {
        Row {
            key,
            label,
            human: human.into(),
            machine: machine.into(),
        }
    }
}

pub struct Ui {
    mode: Mode,
    out: Box<dyn Write>,
    err: Box<dyn Write>,
}

impl Ui {
    /// Machine mode when `--quiet` or stdout is not a terminal.
    pub fn from_cli(quiet: bool) -> Ui {
        let mode = if quiet || !std::io::stdout().is_terminal() {
            Mode::Machine
        } else {
            Mode::Human
        };
        Ui {
            mode,
            out: Box::new(std::io::stdout()),
            err: Box::new(std::io::stderr()),
        }
    }

    /// Test/injection constructor.
    #[cfg(test)]
    pub fn from_parts(mode: Mode, out: Box<dyn Write>, err: Box<dyn Write>) -> Ui {
        Ui { mode, out, err }
    }

    /// Build a progress view for this run (inert in Machine mode).
    pub fn progress(&self, plan: progress::ProgressPlan) -> progress::Progress {
        progress::Progress::new(self.mode, plan)
    }

    /// A voice aside (opener/closer/note). Human only.
    pub fn say(&mut self, line: &str) {
        if self.mode == Mode::Human {
            let _ = writeln!(self.out, "{line}");
        }
    }

    /// A terminal outcome: voice for humans, a stable token for scripts.
    pub fn line(&mut self, human: &str, machine: &str) {
        let _ = match self.mode {
            Mode::Human => writeln!(self.out, "{human}"),
            Mode::Machine => writeln!(self.out, "{machine}"),
        };
    }

    /// A labeled data block. Human aligns `label:` columns; Machine emits `key=value`.
    pub fn rows(&mut self, rows: &[Row]) {
        match self.mode {
            Mode::Human => {
                let w = rows.iter().map(|r| r.label.len()).max().unwrap_or(0) + 1;
                for r in rows {
                    let label = format!("{}:", r.label);
                    let _ = writeln!(self.out, "{label:<w$} {}", r.human, w = w);
                }
            }
            Mode::Machine => {
                for r in rows {
                    let _ = writeln!(self.out, "{}={}", r.key, r.machine);
                }
            }
        }
    }

    /// A hex dump (raw SFDP). Same 16-per-row layout in both modes.
    pub fn hexdump(&mut self, base: usize, bytes: &[u8]) {
        for (i, chunk) in bytes.chunks(16).enumerate() {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
            let _ = writeln!(self.out, "  {:04X}: {}", base + i * 16, hex.join(" "));
        }
    }

    /// Render a failure to stderr; return the process exit code.
    pub fn fail(&mut self, e: &NorbertError) -> ExitCode {
        let (human, machine, code): (String, String, u8) = match e {
            NorbertError::NoFlash => (voice::no_flash().to_string(), "FAIL: no chip".into(), 1),
            NorbertError::Unsupported(j) => (
                voice::unsupported(*j),
                format!("FAIL: unsupported {:02X} {:02X} {:02X}", j[0], j[1], j[2]),
                1,
            ),
            NorbertError::VerifyMismatch { addr } => (
                voice::verify_fail(*addr),
                format!("FAIL: verify @0x{addr:06X}"),
                1,
            ),
            NorbertError::Protected => {
                (voice::protected().to_string(), "FAIL: protected".into(), 1)
            }
            NorbertError::Timeout => (voice::timeout().to_string(), "FAIL: timeout".into(), 1),
            NorbertError::NotDetected => (
                voice::not_detected().to_string(),
                "FAIL: not detected".into(),
                1,
            ),
            NorbertError::Cancelled => (
                voice::cancelled().to_string(),
                "FAIL: cancelled".into(),
                130,
            ),
            NorbertError::Other(err) => (format!("{err:#}"), format!("FAIL: {err}"), 1),
        };
        let _ = match self.mode {
            Mode::Human => writeln!(self.err, "{human}"),
            Mode::Machine => writeln!(self.err, "{machine}"),
        };
        ExitCode::from(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn say_dropped_in_machine_but_shown_in_human() {
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);
        ui.say("Hmm.");
        assert_eq!(out.contents(), "");
        let (mut ui, out, _e) = Ui::captured(Mode::Human);
        ui.say("Hmm.");
        assert_eq!(out.contents(), "Hmm.\n");
    }

    #[test]
    fn line_picks_the_right_channel() {
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);
        ui.line("Found it.", "EF4018");
        assert_eq!(out.contents(), "EF4018\n");
        let (mut ui, out, _e) = Ui::captured(Mode::Human);
        ui.line("Found it.", "EF4018");
        assert_eq!(out.contents(), "Found it.\n");
    }

    #[test]
    fn rows_align_in_human_and_kv_in_machine() {
        let rows = [
            Row::split("capacity", "capacity", "16384 KiB", "16777216"),
            Row::split("page", "page", "256 B", "256"),
        ];
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);
        ui.rows(&rows);
        assert_eq!(out.contents(), "capacity=16777216\npage=256\n");
        let (mut ui, out, _e) = Ui::captured(Mode::Human);
        ui.rows(&rows);
        assert_eq!(out.contents(), "capacity: 16384 KiB\npage:     256 B\n");
    }

    #[test]
    fn fail_is_voice_in_human_and_token_in_machine() {
        let (mut ui, _o, err) = Ui::captured(Mode::Machine);
        let _ = ui.fail(&NorbertError::Protected);
        assert_eq!(err.contents(), "FAIL: protected\n");
        let (mut ui, _o, err) = Ui::captured(Mode::Human);
        let _ = ui.fail(&NorbertError::Protected);
        assert_eq!(err.contents(), format!("{}\n", voice::protected()));
    }

    #[test]
    fn fail_cancelled_yields_exit_130() {
        // The Ctrl-C exit code is a script-facing contract; pin it.
        let (mut ui, _o, _e) = Ui::captured(Mode::Machine);
        assert_eq!(ui.fail(&NorbertError::Cancelled), ExitCode::from(130));
        let (mut ui, _o, _e) = Ui::captured(Mode::Machine);
        assert_eq!(ui.fail(&NorbertError::Protected), ExitCode::from(1));
    }

    #[test]
    fn hexdump_wraps_at_16_and_addresses_from_base() {
        // hexdump's base != 0 path is unreachable from production (sfdp calls
        // hexdump(0, ..)), so this unit test is the only guard on its arithmetic.
        let (mut ui, out, _e) = Ui::captured(Mode::Human);
        let bytes: Vec<u8> = (0..20).collect(); // 16 + 4 → two rows
        ui.hexdump(0x100, &bytes);
        assert_eq!(
            out.contents(),
            "  0100: 00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F\n\
             \x20 0110: 10 11 12 13\n",
        );
    }
}
