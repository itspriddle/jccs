mod config;
mod input;

use std::{
    io::{self, IsTerminal},
    process::ExitCode,
};

use anyhow::Result;

use crate::input::StatusInput;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("jccs: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let _config = config::load()?;

    // Skip reading when run interactively so the binary doesn't hang
    // waiting for input that Claude Code would normally pipe in.
    let stdin = io::stdin();
    let _input = if stdin.is_terminal() {
        StatusInput::default()
    } else {
        input::read(stdin.lock())?
    };

    println!("hello world");
    Ok(())
}
