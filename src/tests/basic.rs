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
const GUEST_PIO_VMPORT_BIN: &[u8] = include_bytes!("../../payload/guest_pio_vmport.bin");
const GUEST_MMIO_BIN: &[u8] = include_bytes!("../../payload/guest_mmio.bin");
const GUEST_MMIO_REGS_BIN: &[u8] = include_bytes!("../../payload/guest_mmio_regs.bin");
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

// Test MMIO emulation via IOAPIC register accesses.
// Verifies GPR loading/storing, destination/source register mapping,
// and EFLAGS preservation across the handle_mmio code path.
#[test_fn()]
pub(crate) fn test_mmio_guest_bin() -> Result<()> {
    let tmp_dir = tempfile::tempdir().context("failed to create temp dir")?;
    let guest_bin_path = tmp_dir.path().join("guest_mmio.bin");
    fs::write(&guest_bin_path, GUEST_MMIO_BIN).context("failed to write guest binary")?;
    let payload = QemuPayload::GuestBin(guest_bin_path);
    let cfg = QemuConfig::new(&tmp_dir, &payload);
    let mut process = QemuProcess::spawn(cfg).context("failed to spawn QEMU process")?;

    let status = process
        .qmp()
        .execute(&qmp::query_status {})
        .context("query_status failed")?;
    debug!("VM status: {:?}", status.status);

    // Expected: "ABCDE MMIO_OK"
    // A = basic MMIO read/write
    // B = GPR preservation across MMIO
    // C = MMIO read into different destination GPRs
    // D = MMIO write from different source GPRs (EBX vs ECX)
    // E = EFLAGS preservation
    let expected_output = ExpectedOutput::SubString("ABCDE MMIO_OK".into());
    process
        .poll_line(expected_output)
        .context("MMIO emulation test failed")?;

    Ok(())
}

// Test PIO with cpu_synchronize_state interaction via VMPort.
//
// VMPort (I/O port 0x5658) calls cpu_synchronize_state() during a port read,
// which pulls all vCPU registers into QEMU's internal state and sets the dirty
// flag. The vmport command handler then modifies GPRs directly on QEMU-side
// state (e.g. setting EBX). If the PIO fast-path handler incorrectly clears
// the dirty flag after writing only RIP+RAX to the hypervisor, the EBX change
// will be lost and the guest sees stale register values.
//
// Tests:
//   A = CMD_GETVERSION: EBX must change to VMPORT_MAGIC
//   B = CMD_GETRAMSIZE: EBX must change to 0x1177
//   C = Non-vmport GPRs (ESI, EDI, EBP) survive the round-trip
#[test_fn()]
pub(crate) fn test_pio_vmport() -> Result<()> {
    let tmp_dir = tempfile::tempdir().context("failed to create temp dir")?;
    let guest_bin_path = tmp_dir.path().join("guest_pio_vmport.bin");
    fs::write(&guest_bin_path, GUEST_PIO_VMPORT_BIN).context("failed to write guest binary")?;
    let payload = QemuPayload::GuestBin(guest_bin_path);
    let cfg = QemuConfig::new(&tmp_dir, &payload);
    let mut process = QemuProcess::spawn(cfg).context("failed to spawn QEMU process")?;

    let status = process
        .qmp()
        .execute(&qmp::query_status {})
        .context("query_status failed")?;
    debug!("VM status: {:?}", status.status);

    let expected_output = ExpectedOutput::SubString("ABC VMPORT_OK".into());
    process
        .poll_line(expected_output)
        .context("VMPort PIO dirty-flag test failed")?;

    Ok(())
}

#[test_fn(machine = {Machine::Pc, Machine::Q35}, smp = {1, 2, 4}, cpu = [CpuModel::Qemu64, CpuModel::Host])]
pub(crate) fn test_kernel_boot(machine: Machine, smp: u8, cpu: Option<CpuModel>) -> Result<()> {
    let tmp_dir = tempfile::tempdir().context("failed to create temp dir")?;
    let payload = QemuPayload::Kernel {
        kernel: KERNEL.into(),
        initrd: None,
    };
    let mut cfg = QemuConfig::new(&tmp_dir, &payload)
        .with_machine(machine)
        .with_smp(smp);

    if let Some(cpu) = cpu {
        cfg = cfg.with_cpu_model(cpu);
    }
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

// Test MMIO register get/set for all GPRs and EFLAGS.
// Exercises the full register load → emulate → store cycle through the
// mshv_load_regs / mshv_store_regs path, catching mapping bugs between
// the VP register page layout and QEMU's internal register indices.
//
// MSHV-only: On KVM the IOAPIC is emulated in-kernel (version 0x11) rather
// than in QEMU userspace (version 0x20), so the version checks fail and,
// more fundamentally, MMIO accesses don't exit to QEMU at all — they never
// exercise the emulate_instruction → load/store_regs code path.
// However, the test itself still passes on KVM because it uses a dynamic
// reference value read at startup rather than a hardcoded version.
//
// Tests (see src/mmio_regs.c for detailed documentation):
//   A = GPR preservation (EBX..EBP survive MMIO read)
//   B = MMIO read into EAX
//   C = MMIO read into EBX
//   D = MMIO read into ECX
//   E = MMIO read into EDX
//   F = MMIO write from EBX/ECX source registers
//   G = EFLAGS preservation (CF, ZF, SF)
//   H = ESP preservation
//   I = Multi-cycle stability (16 iterations)
#[test_fn()]
pub(crate) fn test_mmio_regs() -> Result<()> {
    let tmp_dir = tempfile::tempdir().context("failed to create temp dir")?;
    let guest_bin_path = tmp_dir.path().join("guest_mmio_regs.bin");
    fs::write(&guest_bin_path, GUEST_MMIO_REGS_BIN).context("failed to write guest binary")?;
    let payload = QemuPayload::GuestBin(guest_bin_path);
    let cfg = QemuConfig::new(&tmp_dir, &payload);
    let mut process = QemuProcess::spawn(cfg).context("failed to spawn QEMU process")?;

    let status = process
        .qmp()
        .execute(&qmp::query_status {})
        .context("query_status failed")?;
    debug!("VM status: {:?}", status.status);

    let expected_output = ExpectedOutput::SubString("ABCDEFGHI REGS_OK".into());
    process
        .poll_line(expected_output)
        .context("MMIO register test failed")?;

    Ok(())
}
