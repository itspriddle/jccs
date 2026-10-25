mod config;
mod git;
mod input;
mod install;
mod status;
mod style;
mod template;
mod vars;

use std::{
    env,
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use crate::{
    config::{Config, SegmentSpec},
    input::StatusInput,
    status::{Env, Status},
    style::ColorMode,
};

/// Josh's Claude Code Statusline. Reads Claude Code's statusline JSON on
/// stdin and prints the line described by `~/.config/jccs/config.toml`.
#[derive(Parser)]
#[command(version, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    render: RenderArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Render the statusline (the default).
    Render(RenderArgs),
    /// Print every available value as `name = value`.
    Vars {
        /// Config file to use instead of the default.
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },
    /// Write a documented config file, with every option set to its
    /// default, to `~/.config/jccs/config.toml`.
    Configure {
        /// Overwrite an existing config file.
        #[arg(short, long)]
        force: bool,
    },
    /// Set jccs as the statusline in Claude Code's settings.json and create
    /// the config file if it is missing.
    Install {
        /// Replace an existing statusLine and config file.
        #[arg(short, long)]
        overwrite: bool,
        /// Don't ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Remove the jccs statusLine from Claude Code's settings.json and delete
    /// the config directory.
    Uninstall {
        /// Don't ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args)]
struct RenderArgs {
    /// Config file to use instead of the default.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Render this single template instead of the configured segments.
    #[arg(long, value_name = "TEMPLATE")]
    format: Option<String>,
    /// Override `color_mode` (and `NO_COLOR`).
    #[arg(long, value_enum, value_name = "MODE")]
    color_mode: Option<ColorMode>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        None => render(cli.render),
        Some(Command::Render(args)) => render(args),
        Some(Command::Vars { config }) => print_vars(config.as_deref()),
        Some(Command::Configure { force }) => configure(force),
        Some(Command::Install { overwrite, yes }) => install::install(overwrite, yes),
        Some(Command::Uninstall { yes }) => install::uninstall(yes),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // Stdout, not stderr: Claude Code only displays stdout, so a
            // broken config would otherwise show up as a blank statusline.
            println!("jccs: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn render(args: RenderArgs) -> Result<()> {
    let mut config = config::load(args.config.as_deref())?;

    if let Some(format) = args.format {
        config.segments = vec![SegmentSpec::Format(format)];
    }
    if let Some(mode) = args.color_mode {
        config.color_mode = mode;
    } else if env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        config.color_mode = ColorMode::None;
    }

    let layout = config.compile()?;
    let status = read_status(&config)?;
    println!("{}", layout.render(&status.vars()));
    Ok(())
}

fn print_vars(config_path: Option<&Path>) -> Result<()> {
    let config = config::load(config_path)?;
    for (name, value) in read_status(&config)?.vars() {
        println!("{name} = {value}");
    }
    Ok(())
}

fn configure(force: bool) -> Result<()> {
    let path = config::config_path()
        .context("cannot determine config directory; set HOME or XDG_CONFIG_HOME")?;
    config::write_template(&path, force)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn read_status(config: &Config) -> Result<Status> {
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

    Ok(Status::from_input(&input, &env, config))
}

fn unix_now() -> Result<i64> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs();
    Ok(i64::try_from(secs)?)
}
