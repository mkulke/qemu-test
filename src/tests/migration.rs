use crate::cloud_init::{CloudInitDisk, GUEST_USER};
use crate::process::CpuModel as Cpu;
use crate::process::{ExpectedOutput, Machine, QemuConfig, QemuPayload, QemuProcess, RtcClock};
use crate::ssh::ssh_command;
use crate::tests::full_os::OS_READY_PATTERN;
use crate::util::{NetConfig, allocate_taps, generate_mac};
use anyhow::{Context, Result, ensure};
use log::debug;
use qapi::qmp::{self, RunState};
use regex::Regex;
use std::time::Duration;
use test_macro::test_fn;

const GUEST_BIN: &[u8] = include_bytes!("../../payload/guest.bin");
const GUEST_AVX2_BIN: &[u8] = include_bytes!("../../payload/guest_avx2.bin");
const GUEST_SCALAR_BIN: &[u8] = include_bytes!("../../payload/guest_scalar.bin");
const GUEST_FP_SSE_BIN: &[u8] = include_bytes!("../../payload/guest_fp_sse.bin");
const EXPECTED_OUTPUT: &str = "HELLO FROM GUEST";
const KERNEL: &str = "payload/vmlinuz-virt";
const INITRD: &str = "payload/initrd.img";
const OS_IMAGE: &str = "payload/os-image.qcow2";
const OS_BOOT_TIMEOUT: Duration = Duration::from_secs(60);
const MIGRATION_TIMEOUT: Duration = Duration::from_secs(10);
const MIGRATION_STRESS_TIMEOUT: Duration = Duration::from_secs(60);
const SSH_TIMEOUT: Duration = Duration::from_secs(30);

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

    src.poll_status(RunState::postmigrate, timeout)?;
    debug!("source VM in postmigrate state");

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

// #[test_fn(machine = Machine::Q35, smp = {1, 2, 4}, stress_ng = true)]
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

    let mut ci = CloudInitDisk::new(src_dir.path())?.with_net_config(&src_net);
    if stress_ng {
        let file = (
            "/etc/default/qemu-mshv-selftest-stress",
            // "STRESS_NG_ARGS=\"--cpu 0 --vm 1 --vm-bytes 256M --hdd 1 --timeout 0\"",
            "STRESS_NG_ARGS=\"--cpu 0 --vm 1 --vm-bytes 128M --timeout 0\"",
        );
        ci = ci.with_write_files(&[file]);
    }

    ci.create().context("failed to create cloud-init disk")?;

    let do_ssh = |cmd: &str| {
        ssh_command(
            ci.ssh_key_path(),
            taps.guest_host(),
            22,
            GUEST_USER,
            cmd,
            SSH_TIMEOUT,
        )
    };

    let payload = QemuPayload::DiskImage(OS_IMAGE.into());

    let base_cfg = QemuConfig::new(&src_dir, &payload)
        .with_machine(machine)
        .with_cpu_model(Cpu::Host)
        .with_smp(smp)
        .with_cloud_init(ci.path())
        .with_net(src_net)
        .with_rtc_clock(RtcClock::Vm);

    // Boot source and wait for login prompt
    let mut src = QemuProcess::spawn(base_cfg.clone()).context("failed to spawn source VM")?;

    let expected = ExpectedOutput::Pattern(OS_READY_PATTERN.try_into()?);
    src.poll_line_timeout(expected, OS_BOOT_TIMEOUT)
        .context("source VM did not boot")?;
    debug!("source VM booted");

    // Wait for SSH to become available
    do_ssh("true && echo SSH OK")?;
    debug!("source SSH is reachable");

    // Optionally copy and start stress-ng to load the guest during migration
    if stress_ng {
        let cmd = "sudo systemctl start stress-ng && systemctl is-active stress-ng";
        do_ssh(cmd).context("failed to start stress-ng in guest")?;
        debug!("started stress-ng in guest");
    }

    let dst_cfg = base_cfg.with_incoming(&dst_dir).with_net(dst_net);
    let mut dst = QemuProcess::spawn(dst_cfg).context("failed to spawn destination VM")?;

    let timeout = if stress_ng {
        MIGRATION_STRESS_TIMEOUT
    } else {
        MIGRATION_TIMEOUT
    };
    do_migration(&mut src, &mut dst, &mig_sock, timeout)?;
    debug!("migration completed");

    // check whether stress-ng is still running in the guest after migration
    if stress_ng {
        let cmd = "systemctl is-active stress-ng";
        let output = do_ssh(cmd).context("failed to verify stress-ng is active after migration")?;
        ensure!(
            output == "active",
            "stress-ng is not active after migration"
        );
        debug!("stress-ng verified active in guest after migration");
    }

    Ok(())
}

#[test_fn(smp = {1, 2})]
pub(crate) fn test_live_migration_avx2(smp: u8) -> Result<()> {
    let src_dir = tempfile::tempdir().context("failed to create src temp dir")?;
    let dst_dir = tempfile::tempdir().context("failed to create dst temp dir")?;
    let mig_dir = tempfile::tempdir().context("failed to create migration temp dir")?;
    let mig_sock = mig_dir.path().join("migration.sock");

    let guest_bin_path = src_dir.path().join("guest_avx2.bin");
    std::fs::write(&guest_bin_path, GUEST_AVX2_BIN).context("failed to write AVX2 guest binary")?;
    let payload = QemuPayload::GuestBin(guest_bin_path);

    let cfg = QemuConfig::new(&src_dir, &payload)
        .with_cpu_model(Cpu::Host)
        .with_machine(Machine::Pc)
        .with_smp(smp);

    let mut src = QemuProcess::spawn(cfg.clone()).context("failed to spawn source VM")?;

    // Wait for guest to load YMM registers and signal readiness
    src.poll_line(ExpectedOutput::SubString("AVX2:READY".into()))
        .context("source: AVX2 guest did not become ready")?;
    debug!("AVX2 guest ready on source");

    let cfg = cfg.with_incoming(&dst_dir);
    let mut dst = QemuProcess::spawn(cfg).context("failed to spawn dest VM")?;

    do_migration(&mut src, &mut dst, &mig_sock, MIGRATION_TIMEOUT)?;

    // Verify YMM registers survived migration
    dst.poll_line(ExpectedOutput::SubString("AVX2:OK".into()))
        .context("destination: YMM registers not intact after migration")?;
    debug!("AVX2 YMM registers verified after migration");

    Ok(())
}

#[test_fn(smp = {1, 2})]
pub(crate) fn test_live_migration_scalar_state(smp: u8) -> Result<()> {
    let src_dir = tempfile::tempdir().context("failed to create src temp dir")?;
    let dst_dir = tempfile::tempdir().context("failed to create dst temp dir")?;
    let mig_dir = tempfile::tempdir().context("failed to create migration temp dir")?;
    let mig_sock = mig_dir.path().join("migration.sock");

    let guest_bin_path = src_dir.path().join("guest_scalar.bin");
    std::fs::write(&guest_bin_path, GUEST_SCALAR_BIN)
        .context("failed to write scalar guest binary")?;
    let payload = QemuPayload::GuestBin(guest_bin_path);

    let cfg = QemuConfig::new(&src_dir, &payload)
        .with_cpu_model(Cpu::Host)
        .with_machine(Machine::Pc)
        .with_smp(smp);

    let mut src = QemuProcess::spawn(cfg.clone()).context("failed to spawn source VM")?;

    src.poll_line(ExpectedOutput::SubString("SCALAR:READY".into()))
        .context("source: scalar guest did not become ready")?;
    debug!("scalar guest ready on source");

    let cfg = cfg.with_incoming(&dst_dir);
    let mut dst = QemuProcess::spawn(cfg).context("failed to spawn dest VM")?;

    do_migration(&mut src, &mut dst, &mig_sock, MIGRATION_TIMEOUT)?;

    dst.poll_line(ExpectedOutput::SubString("SCALAR:OK".into()))
        .context("destination: scalar CPU state not intact after migration")?;
    debug!("scalar CPU state verified after migration");

    Ok(())
}

#[test_fn(smp = {1, 2})]
pub(crate) fn test_live_migration_fp_sse_state(smp: u8) -> Result<()> {
    let src_dir = tempfile::tempdir().context("failed to create src temp dir")?;
    let dst_dir = tempfile::tempdir().context("failed to create dst temp dir")?;
    let mig_dir = tempfile::tempdir().context("failed to create migration temp dir")?;
    let mig_sock = mig_dir.path().join("migration.sock");

    let guest_bin_path = src_dir.path().join("guest_fp_sse.bin");
    std::fs::write(&guest_bin_path, GUEST_FP_SSE_BIN)
        .context("failed to write FP/SSE guest binary")?;
    let payload = QemuPayload::GuestBin(guest_bin_path);

    let cfg = QemuConfig::new(&src_dir, &payload)
        .with_cpu_model(Cpu::Host)
        .with_machine(Machine::Pc)
        .with_smp(smp);

    let mut src = QemuProcess::spawn(cfg.clone()).context("failed to spawn source VM")?;

    src.poll_line(ExpectedOutput::SubString("FPSSE:READY".into()))
        .context("source: FP/SSE guest did not become ready")?;
    debug!("FP/SSE guest ready on source");

    let cfg = cfg.with_incoming(&dst_dir);
    let mut dst = QemuProcess::spawn(cfg).context("failed to spawn dest VM")?;

    do_migration(&mut src, &mut dst, &mig_sock, MIGRATION_TIMEOUT)?;

    let result = dst
        .poll_line_match_timeout(
            ExpectedOutput::Pattern(Regex::new(r"FPSSE:(OK|FAIL_[A-Z0-9_]+)")?),
            MIGRATION_TIMEOUT,
        )
        .context("destination: FP/SSE state not intact after migration")?;
    ensure!(
        result.contains("FPSSE:OK"),
        "destination: FP/SSE diagnostic failed: {}",
        result.trim_end()
    );
    debug!("FP/SSE state verified after migration");

    Ok(())
}
