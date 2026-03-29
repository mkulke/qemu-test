use anyhow::{Context, Result};
use process::{QemuConfig, QemuPayload, QemuProcess};
use qapi::qmp::{self, RunState};

mod process;

const GUEST_BIN: &[u8] = include_bytes!("../payload/guest.bin");
const KERNEL: &str = "payload/vmlinuz-virt";
const EXPECTED_OUTPUT: &str = "HELLO FROM GUEST";

macro_rules! function_name {
    () => {{
        fn f() {}
        let name = std::any::type_name_of_val(&f);
        name.rsplit("::").nth(1).unwrap()
    }};
}

fn test_simple_guest_bin() -> Result<()> {
    println!("--- {} ---", function_name!());
    let tmp_dir = tempfile::tempdir().context("failed to create temp dir")?;
    let guest_bin_path = tmp_dir.path().join("guest.bin");
    std::fs::write(&guest_bin_path, GUEST_BIN).context("failed to write guest binary")?;
    let payload = QemuPayload::GuestBin(guest_bin_path);
    let cfg = QemuConfig::new(&tmp_dir, &payload);
    let mut process = QemuProcess::spawn(cfg).context("failed to spawn QEMU process")?;

    let status = process
        .qmp()
        .execute(&qmp::query_status {})
        .context("query_status failed")?;
    println!("VM status: {:?}", status.status);

    process
        .poll_line(EXPECTED_OUTPUT)
        .context("expected output not found")?;
    println!("✓ guest serial output verified!");

    Ok(())
}

fn test_kernel_boot() -> Result<()> {
    println!("--- {} ---", function_name!());
    let tmp_dir = tempfile::tempdir().context("failed to create temp dir")?;
    let payload = QemuPayload::Kernel(KERNEL.into());
    let cfg = QemuConfig::new(&tmp_dir, &payload);
    let mut process = QemuProcess::spawn(cfg).context("failed to spawn QEMU process")?;

    let status = process
        .qmp()
        .execute(&qmp::query_status {})
        .context("query_status failed")?;
    println!("VM status: {:?}", status.status);

    process
        .poll_line("Hypervisor detected")
        .context("kernel boot output not found")?;
    println!("✓ kernel boot verified!");

    Ok(())
}

fn test_live_migration() -> Result<()> {
    println!("--- {} ---", function_name!());

    let src_dir = tempfile::tempdir().context("failed to create src temp dir")?;
    let dst_dir = tempfile::tempdir().context("failed to create dst temp dir")?;
    let mig_dir = tempfile::tempdir().context("failed to create migration temp dir")?;
    let mig_sock = mig_dir.path().join("migration.sock");

    let guest_bin_path = src_dir.path().join("guest.bin");
    std::fs::write(&guest_bin_path, GUEST_BIN).context("failed to write guest binary")?;
    let payload = QemuPayload::GuestBin(guest_bin_path);

    // Start source VM and verify it's running
    let cfg = QemuConfig::new(&src_dir, &payload);
    let mut src = QemuProcess::spawn(cfg).context("failed to spawn source VM")?;
    let status = src
        .qmp()
        .execute(&qmp::query_status {})
        .context("query_status failed")?;
    println!("source VM status: {:?}", status.status);

    // Start destination VM in incoming migration mode
    let cfg = QemuConfig::new_incoming(&dst_dir, &payload);
    let mut dst = QemuProcess::spawn(cfg).context("failed to spawn dest VM")?;
    let dst_status = dst
        .qmp()
        .execute(&qmp::query_status {})
        .context("dest: query_status failed")?;
    println!("destination VM status: {:?}", dst_status.status);

    // Tell destination to listen for migration on a unix socket
    dst.qmp()
        .execute(&qmp::migrate_incoming {
            uri: Some(format!("unix:{}", mig_sock.display())),
            channels: None,
            exit_on_error: None,
        })
        .context("dest: migrate_incoming failed")?;
    println!("destination VM listening for migration");

    // Initiate migration from source
    src.qmp()
        .execute(&qmp::migrate {
            uri: Some(format!("unix:{}", mig_sock.display())),
            channels: None,
            detach: Some(true),
            resume: None,
        })
        .context("source: migrate failed")?;
    println!("source VM migration initiated");

    // Poll destination status until it transitions to running
    let expected_state = RunState::running;
    dst.poll_status(expected_state)?;
    println!("destination VM status: {:?}", expected_state);

    // Verify destination is healthy by reading serial output
    dst.poll_line(EXPECTED_OUTPUT)
        .context("destination: guest not producing serial output after migration")?;
    println!("✓ migration verified, destination guest is alive");

    Ok(())
}

fn main() -> Result<()> {
    test_simple_guest_bin()?;
    test_kernel_boot()?;
    test_live_migration()?;

    Ok(())
}
