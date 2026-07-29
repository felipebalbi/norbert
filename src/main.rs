#![deny(clippy::print_stdout, clippy::print_stderr)]
//! Norbert binary entry point.
//!
//! All logic lives in the `norbert` library; this shim only starts the async
//! runtime and forwards to [`norbert::run`].

use std::process::ExitCode;

// multi_thread is REQUIRED: the HAL bridges blocking GPIO/SPI-config calls via
// tokio::task::block_in_place, which panics on a current-thread runtime.
#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    norbert::run().await
}
