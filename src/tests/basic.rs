use crate::config::CONFIG;
use crate::process::{
    Accelerator, CpuModel, ExpectedOutput, Machine, QemuConfig, QemuPayload, QemuProcess,
};
use anyhow::{Context, Result};
use log::debug;
use qapi::qmp;
use regex::Regex;
use std::fs;
use test_macro::test_fn;

const GUEST_BIN: &[u8] = include_bytes!("../../payload/guest.bin");
const GUEST_PIO_STR_BIN: &[u8] = include_bytes!("../../payload/guest_pio_str.bin");
const KERNEL: &str = "payload/vmlinuz-virt";
const EXPECTED_OUTPUT: &str = "HELLO FROM GUEST";
const PIO_STR_PREFIX: usize = 13; // 'A' bytes before the insd target (0x10FF0..0x10FFC)
const PIO_STR_WRITE_D: usize = 2; // 'D' bytes from the page-crossing insd (0x10FFD..0x10FFE)
const PIO_STR_WRITE_C: usize = 2; // 'C' bytes from the page-crossing insd (0x10FFF..0x11000)
const PIO_STR_SUFFIX: usize = 16; // 'A' bytes after page boundary (0x11001..0x11010)
const PIO_STR_READ_X: usize = 2; // 'X' bytes from page-crossing outsd readback (0x11011..0x11012)
const PIO_STR_READ_Y: usize = 2; // 'Y' bytes from page-crossing outsd readback (0x11013..0x11014)

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

// https://github.com/microsoft/qemu/issues/17
#[test_fn()]
pub(crate) fn test_pio_str_guest_bin() -> Result<()> {
    let tmp_dir = tempfile::tempdir().context("failed to create temp dir")?;
    let guest_bin_path = tmp_dir.path().join("guest_pio_str.bin");
    fs::write(&guest_bin_path, GUEST_PIO_STR_BIN).context("failed to write guest binary")?;
    let payload = QemuPayload::GuestBin(guest_bin_path);
    let cfg = QemuConfig::new(&tmp_dir, &payload);
    let mut process = QemuProcess::spawn(cfg).context("failed to spawn QEMU process")?;

    let status = process
        .qmp()
        .execute(&qmp::query_status {})
        .context("query_status failed")?;
    debug!("VM status: {:?}", status.status);

    // Verify both page-crossing write (write_memory) and page-crossing read
    // (read_memory): buffer should contain 13 A's, 2 D's + 2 C's (INSD
    // page-crossing write via write_memory), 16 A's, 2 X's + 2 Y's (OUTSD
    // page-crossing read via read_memory, round-tripped through PCI config
    // register), then the marker.
    // If write_memory fails to cross the page boundary, D/C counts won't match.
    // If read_memory fails to cross the page boundary, X/Y counts won't match.
    // NOTE: The `poll_line` method trims the end, so we can't match the `$`.
    let pattern = Regex::new(&format!(
        "^A{{{PIO_STR_PREFIX}}}D{{{PIO_STR_WRITE_D}}}C{{{PIO_STR_WRITE_C}}}A{{{PIO_STR_SUFFIX}}}X{{{PIO_STR_READ_X}}}Y{{{PIO_STR_READ_Y}}}HELLO VIA OUTSB"
    ))
    .context("failed to compile regex")?;
    let expected_output = ExpectedOutput::Pattern(pattern);
    process
        .poll_line(expected_output)
        .context("page-crossing read_memory/write_memory verification failed")?;

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
