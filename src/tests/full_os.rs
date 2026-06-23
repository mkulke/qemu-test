use crate::cloud_init::{CloudInitDisk, GUEST_USER};
use crate::process::{
    CpuModel as Cpu, ExpectedOutput, Machine, QemuConfig, QemuPayload, QemuProcess,
};
use crate::ssh::{ssh_command, ssh_fire_and_forget};
use crate::util::NetConfig;
use anyhow::{Context, Result, ensure};
use log::debug;
use qapi::qmp;
use std::time::Duration;
use test_macro::test_fn;

const OS_IMAGE: &str = "payload/os-image.qcow2";
const OVMF_CODE: &str = "payload/OVMF_CODE.fd";
const BOOT_TIMEOUT: Duration = Duration::from_secs(45);
const REBOOT_TIMEOUT: Duration = Duration::from_secs(60);
const SSH_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const OS_READY_PATTERN: &str = r"^CentOS Stream \d+";
// pub(crate) const OS_READY_PATTERN: &str = r"^Fedora Linux \d+";

#[test_fn(
    cpu = {Cpu::Host, Cpu::HaswellV2},
    machine = {Machine::Pc, Machine::Q35},
    smp = {1, 2, 4},
    ovmf = [],
    io_thread = {true, false},
)]
// OVMF requires UEFI support, which is not available on Machine::Pc
#[test_fn(
    cpu = {Cpu::Host, Cpu::HaswellV2},
    machine = Machine::Q35,
    smp = {1, 2, 4},
    ovmf = [OVMF_CODE],
    io_thread = {true, false},
)]
pub(crate) fn test_os_boot(
    cpu: Cpu,
    machine: Machine,
    smp: u8,
    ovmf: Option<&str>,
    io_thread: bool,
) -> Result<()> {
    let tmp_dir = tempfile::tempdir().context("failed to create temp dir")?;

    let net_config = NetConfig::user_net();
    let mut ci = CloudInitDisk::new(tmp_dir.path())?.with_net_config(&net_config);
    ci.create().context("failed to create cloud-init disk")?;

    let payload = QemuPayload::DiskImage(OS_IMAGE.into());
    let mut cfg = QemuConfig::new(&tmp_dir, &payload)
        .with_machine(machine)
        .with_smp(smp)
        .with_cloud_init(ci.path())
        .with_net(net_config)
        .with_cpu_model(cpu);
    if let Some(path) = ovmf {
        cfg = cfg.with_ovmf(path.into());
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
        ci.ssh_key_path(),
        "localhost",
        ssh_port,
        GUEST_USER,
        "hostname",
        SSH_TIMEOUT,
    )?;
    debug!("guest hostname: {hostname}");
    ensure!(hostname == "cloud", "unexpected hostname: {hostname}");

    Ok(())
}

#[test_fn(smp = {1, 2})]
pub(crate) fn test_os_reboot(smp: u8) -> Result<()> {
    let tmp_dir = tempfile::tempdir().context("failed to create temp dir")?;

    let net_config = NetConfig::user_net();
    let mut ci = CloudInitDisk::new(tmp_dir.path())?.with_net_config(&net_config);
    ci.create().context("failed to create cloud-init disk")?;

    let payload = QemuPayload::DiskImage(OS_IMAGE.into());
    let cfg = QemuConfig::new(&tmp_dir, &payload)
        .with_smp(smp)
        .with_cpu_model(Cpu::HaswellV2)
        .with_cloud_init(ci.path())
        .with_net(net_config)
        .with_allow_reboot();
    let mut process = QemuProcess::spawn(cfg).context("failed to spawn QEMU process")?;

    let ssh_port = process.ssh_port()?;

    // Wait for initial boot
    let ready_pattern = ExpectedOutput::Pattern(OS_READY_PATTERN.try_into()?);
    process
        .poll_line_timeout(ready_pattern, BOOT_TIMEOUT)
        .context("initial boot did not complete")?;
    debug!("initial boot complete");

    // Verify SSH works before reboot
    ssh_command(
        ci.ssh_key_path(),
        "localhost",
        ssh_port,
        GUEST_USER,
        "hostname",
        SSH_TIMEOUT,
    )
    .context("SSH failed before reboot")?;

    // Issue reboot
    debug!("issuing reboot via SSH");
    ssh_fire_and_forget(
        ci.ssh_key_path(),
        "localhost",
        ssh_port,
        GUEST_USER,
        "sudo reboot",
    );

    // Wait for login prompt to reappear on serial (proves the guest rebooted)
    let ready_pattern = ExpectedOutput::Pattern(OS_READY_PATTERN.try_into()?);
    process
        .poll_line_timeout(ready_pattern, REBOOT_TIMEOUT)
        .context("guest did not come back after reboot")?;
    debug!("guest rebooted successfully");

    // Verify the guest is functional after reboot
    let uptime = ssh_command(
        ci.ssh_key_path(),
        "localhost",
        ssh_port,
        GUEST_USER,
        "cat /proc/uptime",
        SSH_TIMEOUT,
    )
    .context("SSH failed after reboot")?;
    let uptime_secs: f64 = uptime
        .split_whitespace()
        .next()
        .context("empty uptime output")?
        .parse()
        .context("failed to parse uptime")?;
    debug!("uptime after reboot: {uptime_secs:.1}s");
    ensure!(
        uptime_secs < 120.0,
        "uptime too high after reboot ({uptime_secs:.1}s), reboot may not have occurred"
    );

    Ok(())
}
