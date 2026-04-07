use crate::config::CONFIG;
use crate::process::{
    Accelerator, CpuModel, ExpectedOutput, Machine, QemuConfig, QemuPayload, QemuProcess,
};
use anyhow::{Context, Result};
use log::debug;
use qapi::qmp;
use std::fs;
use test_macro::test_fn;

const GUEST_BIN: &[u8] = include_bytes!("../../payload/guest.bin");
const KERNEL: &str = "payload/vmlinuz-virt";
const EXPECTED_OUTPUT: &str = "HELLO FROM GUEST";

#[test_fn()]
pub(crate) fn test_simple_guest_bin() -> Result<()> {
    let tmp_dir = tempfile::tempdir().context("failed to create temp dir")?;
    let guest_bin_path = tmp_dir.path().join("guest.bin");
    fs::write(&guest_bin_path, GUEST_BIN).context("failed to write guest binary")?;
    let payload = QemuPayload::GuestBin(guest_bin_path);
    let cfg = QemuConfig::new(&tmp_dir, &payload);
    let mut process = QemuProcess::spawn(cfg).context("failed to spawn QEMU process")?;

    let status = process
        .qmp()
        .execute(&qmp::query_status {})
        .context("query_status failed")?;
    debug!("VM status: {:?}", status.status);

    let expected_output = ExpectedOutput::SubString(EXPECTED_OUTPUT.into());
    process
        .poll_line(expected_output)
        .context("expected output not found")?;

    Ok(())
}

#[test_fn(machine = {Machine::Pc, Machine::Q35}, smp = {1, 2, 4}, cpu = {CpuModel::Qemu64, CpuModel::Host})]
pub(crate) fn test_kernel_boot(machine: Machine, smp: u8, cpu: CpuModel) -> Result<()> {
    let tmp_dir = tempfile::tempdir().context("failed to create temp dir")?;
    let payload = QemuPayload::Kernel {
        kernel: KERNEL.into(),
        initrd: None,
    };
    let cfg = QemuConfig::new(&tmp_dir, &payload)
        .with_machine(machine)
        .with_smp(smp)
        .with_cpu_model(cpu);
    let mut process = QemuProcess::spawn(cfg).context("failed to spawn QEMU process")?;

    let status = process
        .qmp()
        .execute(&qmp::query_status {})
        .context("query_status failed")?;
    debug!("VM status: {:?}", status.status);

    let hv = match CONFIG.accel()? {
        Accelerator::Kvm => "KVM",
        Accelerator::Mshv => "Microsoft Hyper-V",
    };
    let expected_output = ExpectedOutput::SubString(format!("Hypervisor detected: {hv}"));
    process
        .poll_line(expected_output)
        .context("kernel boot output not found")?;

    Ok(())
}
