use crate::cloud_init::{CloudInitDisk, GUEST_USER};
use crate::process::CpuModel as Cpu;
use crate::process::{ExpectedOutput, Machine, QemuConfig, QemuPayload, QemuProcess};
use crate::tests::full_os::{allocate_port, ssh_command};
use anyhow::{Context, Result};
use log::debug;
use qapi::qmp::{self, RunState};
use std::time::Duration;
use test_macro::test_fn;

const GUEST_BIN: &[u8] = include_bytes!("../../payload/guest.bin");
const EXPECTED_OUTPUT: &str = "HELLO FROM GUEST";
const KERNEL: &str = "payload/vmlinuz-virt";
const INITRD: &str = "payload/initrd.img";
const OS_IMAGE: &str = "payload/os-image.qcow2";
const OS_BOOT_TIMEOUT: Duration = Duration::from_secs(60);
const SSH_TIMEOUT: Duration = Duration::from_secs(30);

fn do_migration(
    src: &mut QemuProcess,
    dst: &mut QemuProcess,
    mig_sock: &std::path::Path,
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

    dst.poll_status(RunState::running)?;
    debug!("destination VM running");

    Ok(())
}

#[test_fn]
pub(crate) fn test_live_migration() -> Result<()> {
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

    do_migration(&mut src, &mut dst, &mig_sock)?;

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

    do_migration(&mut src, &mut dst, &mig_sock)?;

    // Verify init resumed on destination (produces "B" periodically)
    dst.poll_line(ExpectedOutput::SubString("INIT:ALIVE".into()))
        .context("init did not resume on destination after migration")?;
    debug!("init resumed on destination");

    Ok(())
}

#[test_fn(
    machine = {Machine::Pc, Machine::Q35},
    // machine = Machine::Q35,
    smp = {2, 4},
    // smp = 2,
)]
pub(crate) fn test_live_migration_os(machine: Machine, smp: u8) -> Result<()> {
    let src_dir = tempfile::tempdir().context("failed to create src temp dir")?;
    let dst_dir = tempfile::tempdir().context("failed to create dst temp dir")?;
    let mig_dir = tempfile::tempdir().context("failed to create migration temp dir")?;
    let mig_sock = mig_dir.path().join("migration.sock");

    let ci = CloudInitDisk::create(src_dir.path()).context("failed to create cloud-init disk")?;
    // Copy cidata to dst so both VMs can open it without file lock conflicts
    let dst_cidata_path = dst_dir.path().join("cidata.img");
    std::fs::copy(&ci.path, &dst_cidata_path).context("failed to copy cidata to dst")?;

    let src_ssh_port = allocate_port();
    let dst_ssh_port = allocate_port();

    let payload = QemuPayload::DiskImage(OS_IMAGE.into());

    let base_cfg = QemuConfig::new(&src_dir, &payload)
        .with_machine(machine)
        .with_cpu_model(Cpu::Host)
        .with_smp(smp)
        .with_cloud_init(ci.path.clone())
        .with_ssh_port(src_ssh_port);

    // Boot source and wait for login prompt
    let mut src = QemuProcess::spawn(base_cfg.clone()).context("failed to spawn source VM")?;
    let expected = ExpectedOutput::Pattern(r"Ubuntu 22\.04\..* LTS cloud ttyS0".try_into()?);
    src.poll_line_timeout(expected, OS_BOOT_TIMEOUT)
        .context("source VM did not boot")?;
    debug!("source VM booted");

    // Verify SSH on source
    let kernel_before = ssh_command(
        &ci.ssh_key_path,
        src_ssh_port,
        GUEST_USER,
        "uname -r",
        SSH_TIMEOUT,
    )?;
    debug!("source kernel: {kernel_before}");

    // Spawn destination in incoming mode with its own cidata copy and SSH port
    let dst_cfg = base_cfg
        .with_incoming(&dst_dir)
        .with_cloud_init(dst_cidata_path)
        .with_ssh_port(dst_ssh_port);
    let mut dst = QemuProcess::spawn(dst_cfg).context("failed to spawn destination VM")?;

    // Migrate
    do_migration(&mut src, &mut dst, &mig_sock)?;
    debug!("migration completed");

    // Drop source to free resources
    drop(src);
    debug!("source VM terminated");

    // Verify SSH on destination (guest network re-establishes through new user-net)
    let kernel_after = ssh_command(
        &ci.ssh_key_path,
        dst_ssh_port,
        GUEST_USER,
        "uname -r",
        SSH_TIMEOUT,
    )?;
    debug!("destination kernel: {kernel_after}");
    assert_eq!(
        kernel_before, kernel_after,
        "kernel version mismatch after migration"
    );

    Ok(())
}
