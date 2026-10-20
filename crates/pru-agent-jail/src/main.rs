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
    use std::{collections::BTreeMap, os::unix::process::CommandExt, process::Command};

    use anyhow::{Context, Result, anyhow, ensure};
    use landlock::{
        ABI, AccessFs, AccessNet, CompatLevel, Compatible, LandlockStatus, NetPort, Ruleset,
        RulesetAttr, RulesetCreatedAttr, RulesetStatus, path_beneath_rules,
    };
    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
        SeccompRule, TargetArch,
    };

    use super::{Args, MINIMUM_ABI};

    const TARGET_ABI: ABI = ABI::V4;
    // socket(2) type values; the low four bits hold the type, the rest are flags.
    const SOCKET_TYPE_MASK: u64 = 0xf;
    // x86_64 kernels built with the x32 ABI accept every syscall number with this bit set, and a
    // filter keyed on the plain number would not see them.
    #[cfg(target_arch = "x86_64")]
    const X32_SYSCALL_BIT: i64 = 0x4000_0000;

    fn condition(arg: u8, op: SeccompCmpOp, value: u64) -> Result<SeccompCondition> {
        Ok(SeccompCondition::new(
            arg,
            SeccompCmpArgLen::Dword,
            op,
            value,
        )?)
    }

    // Landlock governs TCP connect and bind only, and its rules name a port, not a host. This
    // filter allows only the sockets Landlock can govern plus local Unix sockets: AF_UNIX of any
    // type, and AF_INET or AF_INET6 stream sockets whose protocol is TCP. Every other socket is
    // refused with EPERM: UDP (so no DNS or datagram egress), raw, packet, netlink, vsock, and
    // SCTP or MPTCP streams that Landlock's TCP rule would not cover. io_uring is refused too,
    // because its socket operations would not pass through socket(2).
    fn socket_filter() -> Result<BpfProgram> {
        let unix = libc::AF_UNIX as u64;
        let inet = libc::AF_INET as u64;
        let inet6 = libc::AF_INET6 as u64;
        let mut refused = vec![SeccompRule::new(vec![
            condition(0, SeccompCmpOp::Ne, unix)?,
            condition(0, SeccompCmpOp::Ne, inet)?,
            condition(0, SeccompCmpOp::Ne, inet6)?,
        ])?];
        for domain in [inet, inet6] {
            // seccomp has no masked not-equal, so refuse every type value except SOCK_STREAM.
            for kind in (0..=SOCKET_TYPE_MASK).filter(|kind| *kind != libc::SOCK_STREAM as u64) {
                refused.push(SeccompRule::new(vec![
                    condition(0, SeccompCmpOp::Eq, domain)?,
                    condition(1, SeccompCmpOp::MaskedEq(SOCKET_TYPE_MASK), kind)?,
                ])?);
            }
            refused.push(SeccompRule::new(vec![
                condition(0, SeccompCmpOp::Eq, domain)?,
                condition(2, SeccompCmpOp::Ne, 0)?,
                condition(2, SeccompCmpOp::Ne, libc::IPPROTO_TCP as u64)?,
            ])?);
        }
        let mut rules = BTreeMap::new();
        rules.insert(libc::SYS_socket, refused.clone());
        rules.insert(libc::SYS_io_uring_setup, Vec::new());
        #[cfg(target_arch = "x86_64")]
        {
            rules.insert(libc::SYS_socket | X32_SYSCALL_BIT, refused);
            rules.insert(libc::SYS_io_uring_setup | X32_SYSCALL_BIT, Vec::new());
        }
        let arch = TargetArch::try_from(std::env::consts::ARCH)
            .map_err(|error| anyhow!("unsupported seccomp architecture: {error:?}"))?;
        let filter = SeccompFilter::new(
            rules,
            SeccompAction::Allow,
            SeccompAction::Errno(libc::EPERM as u32),
            arch,
        )?;
        BpfProgram::try_from(filter).context("failed to compile the socket filter")
    }

    // Landlock's restrict_self has already set no_new_privs, which seccomp requires.
    fn apply_socket_filter() -> Result<()> {
        let program = socket_filter()?;
        seccompiler::apply_filter(&program).context("failed to apply the socket filter")
    }

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
        apply_socket_filter()?;

        let abi: ABI = status.landlock.into();
        println!(
            "landlock_abi={} ruleset=fully_enforced seccomp=socket_filter",
            abi as u8
        );
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
        apply_socket_filter()?;

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
