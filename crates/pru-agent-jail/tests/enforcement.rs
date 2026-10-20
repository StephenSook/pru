#![cfg(target_os = "linux")]

use std::{
    net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::Command,
};

use anyhow::{Context, Result, bail};

const CHILD_ENV: &str = "PRU_LANDLOCK_TEST_CHILD";

#[test]
fn probe_reports_fully_enforced_abi_four_or_newer() {
    let output = Command::new(env!("CARGO_BIN_EXE_pru-agent-jail"))
        .arg("--probe")
        .output()
        .expect("run Landlock probe");

    assert!(
        output.status.success(),
        "probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("probe output is UTF-8");
    let abi = stdout
        .trim()
        .strip_prefix("landlock_abi=")
        .and_then(|value| value.strip_suffix(" ruleset=fully_enforced seccomp=socket_filter"))
        .expect("probe output shape")
        .parse::<u8>()
        .expect("numeric Landlock ABI");
    assert!(abi >= 4, "Landlock ABI {abi} is below the required ABI 4");
}

#[test]
fn allows_only_named_writes_and_tcp_destination_port() {
    let allowed_listener = TcpListener::bind("127.0.0.1:0").expect("bind allowed listener");
    let denied_listener = TcpListener::bind("127.0.0.1:0").expect("bind denied listener");
    let allowed_addr = allowed_listener.local_addr().expect("allowed address");
    let denied_addr = denied_listener.local_addr().expect("denied address");

    let root = tempfile::tempdir().expect("temporary root");
    let allowed_dir = root.path().join("allowed");
    let denied_dir = root.path().join("denied");
    std::fs::create_dir_all(&allowed_dir).expect("create allowed directory");
    std::fs::create_dir_all(&denied_dir).expect("create denied directory");
    let allowed_file = allowed_dir.join("created-by-child");
    let denied_file = denied_dir.join("must-not-exist");

    let test_binary = std::env::current_exe().expect("integration test executable path");
    let output = Command::new(env!("CARGO_BIN_EXE_pru-agent-jail"))
        .args(["--write-dir"])
        .arg(&allowed_dir)
        .args(["--connect-port", &allowed_addr.port().to_string(), "--"])
        .arg(test_binary)
        .args(["--exact", "landlock_child", "--nocapture"])
        .env(CHILD_ENV, "1")
        .env("PRU_LANDLOCK_ALLOWED_ADDR", allowed_addr.to_string())
        .env("PRU_LANDLOCK_DENIED_ADDR", denied_addr.to_string())
        .env("PRU_LANDLOCK_ALLOWED_FILE", &allowed_file)
        .env("PRU_LANDLOCK_DENIED_FILE", &denied_file)
        .output()
        .expect("run jailed child");

    assert!(
        output.status.success(),
        "jailed child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("landlock-enforcement-ok"),
        "child did not report enforcement success: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        std::fs::read(&allowed_file).expect("read allowed output"),
        b"allowed"
    );
    assert!(!denied_file.exists());
}

#[test]
fn landlock_child() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    run_child().expect("exercise Landlock restrictions");
}

fn run_child() -> Result<()> {
    let allowed_addr = read_env("PRU_LANDLOCK_ALLOWED_ADDR")?
        .parse::<SocketAddr>()
        .context("invalid allowed address")?;
    let denied_addr = read_env("PRU_LANDLOCK_DENIED_ADDR")?
        .parse::<SocketAddr>()
        .context("invalid denied address")?;
    let allowed_file = PathBuf::from(read_env("PRU_LANDLOCK_ALLOWED_FILE")?);
    let denied_file = PathBuf::from(read_env("PRU_LANDLOCK_DENIED_FILE")?);

    TcpStream::connect(allowed_addr).context("allowed TCP connection failed")?;
    let denied_connect = TcpStream::connect(denied_addr)
        .expect_err("TCP connection to the denied port unexpectedly succeeded");
    ensure_permission_denied(&denied_connect, "denied TCP connection")?;

    // Landlock names a port, not a host, and does not cover UDP, so the socket filter refuses
    // datagram sockets outright: no DNS lookups and no UDP egress from inside the jail.
    for bind in ["127.0.0.1:0", "[::1]:0"] {
        let denied_udp =
            UdpSocket::bind(bind).expect_err("UDP socket creation unexpectedly succeeded");
        ensure_operation_not_permitted(&denied_udp, "denied UDP socket")?;
    }
    // Local Unix sockets stay available; Python's event loop needs a socket pair.
    UnixStream::pair().context("Unix socket pair failed")?;

    std::fs::write(&allowed_file, b"allowed").context("allowed write failed")?;
    let denied_write = std::fs::write(&denied_file, b"denied")
        .expect_err("write outside the allowed directory unexpectedly succeeded");
    ensure_permission_denied(&denied_write, "denied filesystem write")?;

    println!("landlock-enforcement-ok");
    Ok(())
}

fn read_env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("missing {name}"))
}

fn ensure_operation_not_permitted(error: &std::io::Error, operation: &str) -> Result<()> {
    if error.raw_os_error() != Some(1) {
        bail!("{operation} returned {error:?}, expected EPERM")
    }
    Ok(())
}

fn ensure_permission_denied(error: &std::io::Error, operation: &str) -> Result<()> {
    if error.kind() != std::io::ErrorKind::PermissionDenied || error.raw_os_error() != Some(13) {
        bail!("{operation} returned {error:?}, expected EACCES")
    }
    Ok(())
}
