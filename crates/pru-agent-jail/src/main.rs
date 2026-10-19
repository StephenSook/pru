use std::{ffi::OsString, path::PathBuf};

use anyhow::{Result, bail};
use clap::Parser;

#[cfg(target_os = "linux")]
const MINIMUM_ABI: u8 = 4;

#[derive(Debug, Parser)]
#[command(
    name = "pru-agent-jail",
    about = "Run a command with Pru's Landlock agent restrictions"
)]
struct Args {
    /// Print the enforced Landlock ABI and exit.
    #[arg(long)]
    probe: bool,

    /// Directory beneath which writes are allowed. May be repeated.
    #[arg(long = "write-dir", value_name = "PATH")]
    write_dirs: Vec<PathBuf>,

    /// The only TCP destination port the child may connect to.
    #[arg(long, default_value_t = 8787)]
    connect_port: u16,

    /// Command and arguments to execute after applying the jail.
    #[arg(last = true, value_name = "COMMAND")]
    command: Vec<OsString>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.probe {
        if !args.command.is_empty() || !args.write_dirs.is_empty() {
            bail!("--probe cannot be combined with a command or --write-dir");
        }
        return probe();
    }
    if args.command.is_empty() {
        bail!("a command is required after --");
    }
    if args.write_dirs.is_empty() {
        bail!("at least one --write-dir is required");
    }

    run(args)
}

#[cfg(target_os = "linux")]
fn probe() -> Result<()> {
    linux::probe()
}

#[cfg(not(target_os = "linux"))]
fn probe() -> Result<()> {
    bail!("Landlock is only available on Linux")
}

#[cfg(target_os = "linux")]
fn run(args: Args) -> Result<()> {
    linux::run(args)
}

#[cfg(not(target_os = "linux"))]
fn run(_args: Args) -> Result<()> {
    bail!("Landlock is only available on Linux")
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{os::unix::process::CommandExt, process::Command};

    use anyhow::{Context, Result, ensure};
    use landlock::{
        ABI, AccessFs, AccessNet, CompatLevel, Compatible, LandlockStatus, NetPort, Ruleset,
        RulesetAttr, RulesetCreatedAttr, RulesetStatus, path_beneath_rules,
    };

    use super::{Args, MINIMUM_ABI};

    const TARGET_ABI: ABI = ABI::V4;

    pub(super) fn probe() -> Result<()> {
        let status = Ruleset::default()
            .handle_access(AccessNet::ConnectTcp)
            .context("Landlock cannot handle TCP connect access")?
            .create()
            .context("failed to create Landlock probe ruleset")?
            .set_compatibility(CompatLevel::HardRequirement)
            .restrict_self()
            .context("failed to enforce Landlock probe ruleset")?;
        validate_status(status.ruleset, status.landlock)?;

        let abi: ABI = status.landlock.into();
        println!("landlock_abi={} ruleset=fully_enforced", abi as u8);
        Ok(())
    }

    pub(super) fn run(args: Args) -> Result<()> {
        let write_dirs = args
            .write_dirs
            .iter()
            .map(|path| {
                path.canonicalize()
                    .with_context(|| format!("cannot resolve write directory {}", path.display()))
            })
            .collect::<Result<Vec<_>>>()?;
        let write_access = AccessFs::from_write(TARGET_ABI);
        let status = Ruleset::default()
            .handle_access(write_access)
            .context("Landlock cannot handle required filesystem access")?
            .handle_access(AccessNet::ConnectTcp)
            .context("Landlock cannot handle TCP connect access")?
            .create()
            .context("failed to create Landlock ruleset")?
            .add_rules(path_beneath_rules(&write_dirs, write_access))
            .context("failed to add Landlock filesystem rules")?
            .add_rule(NetPort::new(args.connect_port, AccessNet::ConnectTcp))
            .context("failed to add Landlock TCP rule")?
            .set_compatibility(CompatLevel::HardRequirement)
            .restrict_self()
            .context("failed to enforce Landlock ruleset")?;
        validate_status(status.ruleset, status.landlock)?;

        let (program, command_args) = args.command.split_first().context("missing command")?;
        let error = Command::new(program).args(command_args).exec();
        Err(error).with_context(|| format!("failed to execute {}", program.to_string_lossy()))
    }

    fn validate_status(ruleset: RulesetStatus, landlock: LandlockStatus) -> Result<()> {
        ensure!(
            ruleset == RulesetStatus::FullyEnforced,
            "Landlock ruleset is not fully enforced: {ruleset:?}"
        );
        let abi: ABI = landlock.into();
        ensure!(
            abi >= TARGET_ABI,
            "Landlock ABI {actual} is below required ABI {MINIMUM_ABI}",
            actual = abi as u8
        );
        Ok(())
    }
}
