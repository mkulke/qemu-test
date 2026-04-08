use crate::cloud_init::{CloudInitDisk, GUEST_USER};
use crate::process::{
    CpuModel as Cpu, ExpectedOutput, Machine, QemuConfig, QemuPayload, QemuProcess,
};
use crate::util::SshConfig;
use anyhow::{Context, Result, bail, ensure};
use log::debug;
use qapi::qmp;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use test_macro::test_fn;

const OS_IMAGE: &str = "payload/os-image.qcow2";
const OVMF_CODE: &str = "payload/OVMF_CODE.fd";
const BOOT_TIMEOUT: Duration = Duration::from_secs(45);
const SSH_TIMEOUT: Duration = Duration::from_secs(10);
const OS_READY_PATTERN: &str = r"Ubuntu (22|24).04.\d+ LTS cloud ttyS0";

pub(crate) fn ssh_command(
    key_path: &Path,
    port: u16,
    user: &str,
    command: &str,
    timeout: Duration,
) -> Result<String> {
    let start = Instant::now();
    loop {
        let output = Command::new("timeout")
            .args([
                "10",
                "ssh",
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

        if start.elapsed() > timeout {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("SSH failed after {timeout:?}: {stderr}");
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

    let ssh_config = SshConfig::new();
    let ci = CloudInitDisk::create(tmp_dir.path(), ssh_config.mac())
        .context("failed to create cloud-init disk")?;

    let payload = QemuPayload::DiskImage(OS_IMAGE.into());
    let mut cfg = QemuConfig::new(&tmp_dir, &payload)
        .with_machine(machine)
        .with_cpu_model(cpu)
        .with_smp(smp)
        .with_cloud_init(ci.path.clone())
        .with_ssh(ssh_config);
    if ovmf {
        cfg = cfg.with_ovmf(OVMF_CODE.into());
    }
    if io_thread {
        cfg = cfg.with_io_thread();
    }
    let mut process = QemuProcess::spawn(cfg).context("failed to spawn QEMU process")?;

    let ssh_port = process.ssh_port()?;
    debug!("using SSH port {ssh_port}");

    let status = process
        .qmp()
        .execute(&qmp::query_status {})
        .context("query_status failed")?;
    debug!("VM status: {:?}", status.status);

    let expected_output = ExpectedOutput::Pattern(OS_READY_PATTERN.try_into()?);
    process
        .poll_line_timeout(expected_output, BOOT_TIMEOUT)
        .context("cloud-init did not finish")?;

    let hostname = ssh_command(
        &ci.ssh_key_path,
        ssh_port,
        GUEST_USER,
        "hostname",
        SSH_TIMEOUT,
    )?;
    debug!("guest hostname: {hostname}");
    ensure!(hostname == "cloud", "unexpected hostname: {hostname}");

    Ok(())
}
