use crate::cloud_init::{CloudInitDisk, GUEST_USER};
use crate::process::{
    CpuModel as Cpu, ExpectedOutput, Machine, QemuConfig, QemuPayload, QemuProcess,
};
use anyhow::{Context, Result, bail};
use log::debug;
use qapi::qmp;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};
use test_macro::test_fn;

const OS_IMAGE: &str = "payload/os-image.qcow2";
const OVMF_CODE: &str = "payload/OVMF_CODE.fd";
const BOOT_TIMEOUT: Duration = Duration::from_secs(30);
const SSH_TIMEOUT: Duration = Duration::from_secs(10);

static NEXT_PORT: AtomicU16 = AtomicU16::new(10222);

fn allocate_port() -> u16 {
    NEXT_PORT.fetch_add(1, Ordering::Relaxed)
}

fn ssh_command(key_path: &Path, port: u16, user: &str, command: &str) -> Result<String> {
    let start = Instant::now();
    loop {
        let output = Command::new("ssh")
            .args([
                "-i",
                &key_path.to_string_lossy(),
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "BatchMode=yes",
                "-o",
                "LogLevel=ERROR",
                "-p",
                &port.to_string(),
                &format!("{user}@localhost"),
                command,
            ])
            .output()
            .context("failed to run ssh")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            debug!("ssh output: {stdout}");
            return Ok(stdout);
        }

        if start.elapsed() > SSH_TIMEOUT {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("SSH failed after {SSH_TIMEOUT:?}: {stderr}");
        }

        debug!("SSH not ready, retrying...");
        std::thread::sleep(Duration::from_secs(2));
    }
}

#[test_fn(
    cpu = Cpu::Host,
    machine = {Machine::Pc, Machine::Q35},
    smp = {1, 2, 4},
    ovmf = false,
    io_thread = {true, false},
)]
// OVMF requires UEFI support, which is not available on Machine::Pc
#[test_fn(
    cpu = Cpu::Host,
    machine = Machine::Q35,
    smp = {1, 2, 4},
    ovmf = true,
    io_thread = {true, false},
)]
pub(crate) fn test_os_boot(
    cpu: Cpu,
    machine: Machine,
    smp: u8,
    ovmf: bool,
    io_thread: bool,
) -> Result<()> {
    let tmp_dir = tempfile::tempdir().context("failed to create temp dir")?;

    let ci = CloudInitDisk::create(tmp_dir.path()).context("failed to create cloud-init disk")?;
    let ssh_port = allocate_port();
    debug!("using SSH port {ssh_port}");

    let payload = QemuPayload::DiskImage(OS_IMAGE.into());
    let mut cfg = QemuConfig::new(&tmp_dir, &payload)
        .with_machine(machine)
        .with_cpu_model(cpu)
        .with_smp(smp)
        .with_cloud_init(ci.path)
        .with_ssh_port(ssh_port);
    if ovmf {
        cfg = cfg.with_ovmf(OVMF_CODE.into());
    }
    if io_thread {
        cfg = cfg.with_io_thread();
    }
    let mut process = QemuProcess::spawn(cfg).context("failed to spawn QEMU process")?;

    let status = process
        .qmp()
        .execute(&qmp::query_status {})
        .context("query_status failed")?;
    debug!("VM status: {:?}", status.status);

    let expected_output = ExpectedOutput::Pattern(r"Ubuntu 22\.04\..* LTS cloud ttyS0".try_into()?);
    process
        .poll_line_timeout(expected_output, BOOT_TIMEOUT)
        .context("cloud-init did not finish")?;

    let hostname = ssh_command(&ci.ssh_key_path, ssh_port, GUEST_USER, "hostname")?;
    debug!("guest hostname: {hostname}");
    assert_eq!(hostname, "cloud", "unexpected hostname: {hostname}");

    Ok(())
}
