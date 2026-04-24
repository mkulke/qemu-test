use crate::cloud_init::{CloudInitDisk, GUEST_USER};
use crate::process::CpuModel as Cpu;
use crate::process::{ExpectedOutput, Machine, QemuConfig, QemuPayload, QemuProcess, RtcClock};
use crate::tests::full_os::{OS_READY_PATTERN, ssh_command};
use crate::util::{NetConfig, allocate_taps, generate_mac};
use anyhow::{Context, Result, bail, ensure};
use log::debug;
use qapi::qmp::{self, RunState};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::thread::sleep;
use std::time::{Duration, Instant};
use test_macro::test_fn;

const GUEST_BIN: &[u8] = include_bytes!("../../payload/guest.bin");
const EXPECTED_OUTPUT: &str = "HELLO FROM GUEST";
const KERNEL: &str = "payload/vmlinuz-virt";
const INITRD: &str = "payload/initrd.img";
const OS_IMAGE: &str = "payload/os-image.qcow2";
const OS_BOOT_TIMEOUT: Duration = Duration::from_secs(60);
const MIGRATION_TIMEOUT: Duration = Duration::from_secs(10);
const MIGRATION_STRESS_TIMEOUT: Duration = Duration::from_secs(60);
const SSH_TIMEOUT: Duration = Duration::from_secs(30);
const ECHO_PORT: u16 = 7777;
const STRESS_NG_INSTALL_TIMEOUT: Duration = Duration::from_secs(120);
const STRESS_NG_INSTALL_CMD: &str =
    "sudo apt-get update -qq && sudo apt-get install -y -qq stress-ng";
const ECHO_SERVER_CMD: &str = concat!(
    "nohup python3 -c '",
    "import socket; ",
    "s=socket.socket(); ",
    "s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); ",
    "s.bind((\"0.0.0.0\",7777)); ",
    "s.listen(1); ",
    "c,_=s.accept(); ",
    "[c.sendall(d) for d in iter(lambda:c.recv(4096),b\"\")]",
    "' </dev/null >/dev/null 2>&1 &",
);

/// Send a line over a TcpStream and read the echoed response.
fn echo_roundtrip(stream: &mut TcpStream, msg: &str, timeout: Duration) -> Result<String> {
    stream
        .set_write_timeout(Some(timeout))
        .context("set_write_timeout")?;
    stream
        .set_read_timeout(Some(timeout))
        .context("set_read_timeout")?;
    writeln!(stream, "{msg}").context("echo write")?;
    stream.flush().context("echo flush")?;
    let mut reader = BufReader::new(stream.try_clone().context("clone stream")?);
    let mut line = String::new();
    reader.read_line(&mut line).context("echo read")?;
    Ok(line.trim().to_string())
}

/// Connect to the guest echo server, retrying until `timeout`.
fn connect_echo(host: &str, port: u16, timeout: Duration) -> Result<TcpStream> {
    let start = Instant::now();
    loop {
        sleep(Duration::from_millis(200));
        if crate::SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("interrupted");
        }
        match TcpStream::connect_timeout(
            &format!("{host}:{port}").parse().context("parse addr")?,
            Duration::from_secs(2),
        ) {
            Ok(stream) => return Ok(stream),
            Err(e) => {
                if start.elapsed() > timeout {
                    bail!("echo server not reachable after {timeout:?}: {e}");
                }
                debug!("echo connect failed ({e}), retrying...");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

fn do_migration(
    src: &mut QemuProcess,
    dst: &mut QemuProcess,
    mig_sock: &std::path::Path,
    timeout: Duration,
) -> Result<()> {
    dst.qmp()
        .execute(&qmp::migrate_incoming {
            uri: Some(format!("unix:{}", mig_sock.display())),
            channels: None,
            exit_on_error: None,
        })
        .context("dest: migrate_incoming failed")?;
    debug!("destination VM listening for migration");

    src.qmp()
        .execute(&qmp::migrate {
            uri: Some(format!("unix:{}", mig_sock.display())),
            channels: None,
            detach: None,
            resume: None,
        })
        .context("source: migrate failed")?;
    debug!("source VM migration initiated");

    dst.poll_status(RunState::running, timeout)?;
    debug!("destination VM running");

    Ok(())
}

#[test_fn]
pub(crate) fn test_live_migration_simple() -> Result<()> {
    let src_dir = tempfile::tempdir().context("failed to create src temp dir")?;
    let dst_dir = tempfile::tempdir().context("failed to create dst temp dir")?;
    let mig_dir = tempfile::tempdir().context("failed to create migration temp dir")?;
    let mig_sock = mig_dir.path().join("migration.sock");

    let guest_bin_path = src_dir.path().join("guest.bin");
    std::fs::write(&guest_bin_path, GUEST_BIN).context("failed to write guest binary")?;
    let payload = QemuPayload::GuestBin(guest_bin_path);

    let cfg = QemuConfig::new(&src_dir, &payload);
    let mut src = QemuProcess::spawn(cfg.clone()).context("failed to spawn source VM")?;

    let cfg = cfg.with_incoming(&dst_dir);
    let mut dst = QemuProcess::spawn(cfg).context("failed to spawn dest VM")?;

    do_migration(&mut src, &mut dst, &mig_sock, MIGRATION_TIMEOUT)?;

    let expected_output = ExpectedOutput::SubString(EXPECTED_OUTPUT.into());
    dst.poll_line(expected_output)
        .context("destination: guest not producing serial output after migration")?;

    Ok(())
}

#[test_fn(
    cpu = {Cpu::Qemu64, Cpu::Host},
    smp = {1, 2, 4},
)]
pub(crate) fn test_live_migration_kernel(cpu: Cpu, smp: u8) -> Result<()> {
    let src_dir = tempfile::tempdir().context("failed to create src temp dir")?;
    let dst_dir = tempfile::tempdir().context("failed to create dst temp dir")?;
    let mig_dir = tempfile::tempdir().context("failed to create migration temp dir")?;
    let mig_sock = mig_dir.path().join("migration.sock");

    let payload = QemuPayload::Kernel {
        kernel: KERNEL.into(),
        initrd: Some(INITRD.into()),
    };

    // Boot source and wait for init to signal it's alive
    let cfg = QemuConfig::new(&src_dir, &payload)
        .with_cpu_model(cpu)
        .with_smp(smp);
    let mut src = QemuProcess::spawn(cfg.clone()).context("failed to spawn source VM")?;
    src.poll_line(ExpectedOutput::SubString("INIT:READY".into()))
        .context("init did not start on source")?;
    debug!("init active on source");

    // Start destination in incoming mode
    let cfg = cfg.with_incoming(&dst_dir);
    let mut dst = QemuProcess::spawn(cfg).context("failed to spawn dest VM")?;

    do_migration(&mut src, &mut dst, &mig_sock, MIGRATION_TIMEOUT)?;

    // Verify init resumed on destination (produces "B" periodically)
    dst.poll_line(ExpectedOutput::SubString("INIT:ALIVE".into()))
        .context("init did not resume on destination after migration")?;
    debug!("init resumed on destination");

    Ok(())
}

// #[test_fn(machine = Machine::Q35, smp = 1)]
#[test_fn(
    machine = {Machine::Pc, Machine::Q35},
    smp = {1, 2, 4},
    stress_ng = {false, true},
    skip = "requires tap networking",
)]
pub(crate) fn test_live_migration_os(machine: Machine, smp: u8, stress_ng: bool) -> Result<()> {
    let src_dir = tempfile::tempdir().context("failed to create src temp dir")?;
    let dst_dir = tempfile::tempdir().context("failed to create dst temp dir")?;
    let mig_dir = tempfile::tempdir().context("failed to create migration temp dir")?;
    let mig_sock = mig_dir.path().join("migration.sock");

    let mac = generate_mac();
    let taps = allocate_taps().context("failed to allocate tap devices")?;
    debug!(
        "allocated taps: src={}, dst={}, guest={}",
        taps.src(),
        taps.dst(),
        taps.guest_host()
    );
    let src_net = NetConfig::tap(taps.src(), taps.guest_ip(), taps.gateway(), &mac);
    let dst_net = NetConfig::tap(taps.dst(), taps.guest_ip(), taps.gateway(), &mac);

    let ci = CloudInitDisk::create(src_dir.path(), &src_net)
        .context("failed to create cloud-init disk")?;
    // Copy cidata to dst so both VMs can open it without file lock conflicts
    let dst_cidata_path = dst_dir.path().join("cidata.img");
    std::fs::copy(&ci.path, &dst_cidata_path).context("failed to copy cidata to dst")?;

    let payload = QemuPayload::DiskImage(OS_IMAGE.into());

    let base_cfg = QemuConfig::new(&src_dir, &payload)
        .with_machine(machine)
        .with_cpu_model(Cpu::Host)
        .with_smp(smp)
        .with_cloud_init(ci.path.clone())
        .with_net(src_net)
        .with_rtc_clock(RtcClock::Vm);

    // Boot source and wait for login prompt
    let mut src = QemuProcess::spawn(base_cfg.clone()).context("failed to spawn source VM")?;

    let expected = ExpectedOutput::Pattern(OS_READY_PATTERN.try_into()?);
    src.poll_line_timeout(expected, OS_BOOT_TIMEOUT)
        .context("source VM did not boot")?;
    debug!("source VM booted");

    // Wait for SSH to become available
    ssh_command(
        &ci.ssh_key_path,
        taps.guest_host(),
        22,
        GUEST_USER,
        "true",
        SSH_TIMEOUT,
    )
    .context("SSH not reachable on source")?;
    debug!("source SSH is reachable");

    // Start a TCP echo server in the guest using python3 (always available)
    ssh_command(
        &ci.ssh_key_path,
        taps.guest_host(),
        22,
        GUEST_USER,
        ECHO_SERVER_CMD,
        SSH_TIMEOUT,
    )
    .context("failed to start echo server")?;
    debug!("echo server started on guest port {ECHO_PORT}");

    // Optionally install and start stress-ng to load the guest during migration
    if stress_ng {
        ssh_command(
            &ci.ssh_key_path,
            taps.guest_host(),
            22,
            GUEST_USER,
            STRESS_NG_INSTALL_CMD,
            STRESS_NG_INSTALL_TIMEOUT,
        )
        .context("failed to install stress-ng")?;
        debug!("stress-ng installed");

        let vm_bytes_mb = base_cfg.ram_mb() / 4;
        let stress_ng_run_cmd = format!(
            "nohup stress-ng --cpu 0 --vm 1 --vm-bytes {vm_bytes_mb}M --timeout 0 </dev/null >/dev/null 2>&1 &"
        );
        ssh_command(
            &ci.ssh_key_path,
            taps.guest_host(),
            22,
            GUEST_USER,
            &stress_ng_run_cmd,
            SSH_TIMEOUT,
        )
        .context("failed to start stress-ng")?;
        debug!("stress-ng running in guest ({vm_bytes_mb}M vm-bytes)");
    }

    // Open a persistent TCP connection to the echo server
    let mut stream = connect_echo(taps.guest_host(), ECHO_PORT, SSH_TIMEOUT)
        .context("failed to connect to echo server")?;
    debug!("TCP connection established to echo server");

    // Verify echo works before migration
    let reply = echo_roundtrip(&mut stream, "before-migration", Duration::from_secs(5))?;
    ensure!(
        reply == "before-migration",
        "unexpected echo reply before migration: {reply}"
    );
    debug!("echo verified before migration");

    // Spawn destination in incoming mode with its own cidata copy
    let dst_cfg = base_cfg
        .with_incoming(&dst_dir)
        .with_cloud_init(dst_cidata_path)
        .with_net(dst_net);
    let mut dst = QemuProcess::spawn(dst_cfg).context("failed to spawn destination VM")?;

    // Migrate
    let mig_timeout = if stress_ng {
        MIGRATION_STRESS_TIMEOUT
    } else {
        MIGRATION_TIMEOUT
    };
    do_migration(&mut src, &mut dst, &mig_sock, mig_timeout)?;
    debug!("migration completed");

    // Drop source to free resources
    drop(src);
    debug!("source VM terminated");

    // Verify the same TCP connection still works after migration
    let reply = echo_roundtrip(&mut stream, "after-migration", Duration::from_secs(10))?;
    ensure!(
        reply == "after-migration",
        "unexpected echo reply after migration: {reply}"
    );
    debug!("echo verified after migration — TCP connection survived");

    Ok(())
}
