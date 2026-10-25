mod config;
mod git;
mod input;
mod status;
mod vars;

use std::{
    io::{self, IsTerminal},
    path::Path,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

use crate::{
    input::StatusInput,
    status::{Env, Status},
};

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
    let config = config::load()?;

    // Skip reading when run interactively so the binary doesn't hang
    // waiting for input that Claude Code would normally pipe in.
    let stdin = io::stdin();
    let input = if stdin.is_terminal() {
        StatusInput::default()
    } else {
        input::read(stdin.lock())?
    };

    let dir = input
        .workspace
        .as_ref()
        .and_then(|w| w.current_dir.as_deref())
        .or(input.cwd.as_deref())
        .map(Path::new);

    let env = Env {
        now: unix_now()?,
        git_branch: git::branch(dir),
    };

    let status = Status::from_input(&input, &env, &config);
    for (name, value) in status.vars() {
        println!("{name} = {value}");
    }

    Ok(())
}

fn unix_now() -> Result<i64> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs();
    Ok(i64::try_from(secs)?)
}
