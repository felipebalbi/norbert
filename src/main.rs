#![deny(clippy::print_stdout, clippy::print_stderr)]
//! Norbert — a patient SPI-NOR flasher. This entrypoint only parses, builds the
//! `Ui`, and dispatches; personality lives in `voice`, output in `ui`.

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

// multi_thread is REQUIRED: the HAL bridges blocking GPIO/SPI-config calls via
// tokio::task::block_in_place, which panics on a current-thread runtime.
#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
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
