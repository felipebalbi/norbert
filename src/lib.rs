#![deny(clippy::print_stdout, clippy::print_stderr)]
// The crate-root docs intentionally link to private modules; docs.rs renders
// them with `--document-private-items` (see Cargo.toml), so the resulting
// "links to private item" lint is expected — silence it here.
#![allow(rustdoc::private_intra_doc_links)]
//! Norbert — a patient SPI-NOR flasher for the Pico de Gallo USB bridge.
//!
//! Norbert identifies, erases, programs, verifies, and reads back raw SPI-NOR
//! flash chips over a [Pico de Gallo] v1.1 USB-to-SPI bridge. It reads a chip's
//! JEDEC ID and, where available, its SFDP tables to learn the real geometry
//! (page size, address width, erase menu) before touching a single byte —
//! falling back to a small built-in table for parts that do not self-describe.
//! It never guesses.
//!
//! Norbert ships as a thin binary over this library; these docs describe its
//! internal modules rather than a stable public API. The pieces fit together
//! like this:
//!
//! - [`cli`] — the command-line surface (global flags + subcommands). Parsing
//!   only.
//! - [`commands`] — one handler per subcommand, plus the bus-session helpers
//!   ([`with_bus`](commands::with_bus)/[`with_cancel`](commands::with_cancel))
//!   that guarantee the shared bus is always released.
//! - [`flash`] — the hardware-agnostic flashing core: RDID, SFDP probing,
//!   erase planning, page programming, verification.
//! - [`device`] — binds the flashing core to the Pico de Gallo HAL over a fixed
//!   pin map and drives the flash's control lines.
//! - [`profile`] / [`sfdp`] / [`catalog`] — the flash geometry model, the SFDP
//!   byte parser, and the human-name + no-SFDP fallback tables.
//! - [`ui`] / [`voice`] — presentation. `ui` owns all terminal output and the
//!   Human/Machine (`--quiet`) split; `voice` holds the personality text.
//!
//! [`run`] only parses arguments, builds the [`Ui`], and dispatches; personality
//! lives in [`voice`], output in [`ui`].
//!
//! [Pico de Gallo]: https://github.com/felipebalbi/norbert

mod catalog;
mod cli;
mod commands;
mod device;
mod error;
mod flash;
mod profile;
mod sfdp;
mod ui;
mod voice;

#[cfg(test)]
mod testsupport;

use std::process::ExitCode;

use clap::Parser;

use cli::Cli;
use ui::Ui;

/// Parse the command line, build the [`Ui`], and dispatch the selected
/// subcommand, returning the process exit code.
///
/// [`ExitCode::SUCCESS`] is returned for `--version` and the bare
/// no-subcommand help path; command failures are rendered through the UI and
/// mapped to a failure exit code.
pub async fn run() -> ExitCode {
    let cli = Cli::parse();
    let mut ui = Ui::from_cli(cli.quiet);

    if cli.version {
        ui.line(&voice::version(), env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if cli.cmd.is_none() {
        use clap::CommandFactory;
        let _ = Cli::command().print_help();
        return ExitCode::SUCCESS;
    }

    match commands::dispatch(&cli, &mut ui).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => ui.fail(&e),
    }
}
