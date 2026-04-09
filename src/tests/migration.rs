use crate::cloud_init::{CloudInitDisk, GUEST_USER};
use crate::process::CpuModel as Cpu;
use crate::process::{ExpectedOutput, Machine, QemuConfig, QemuPayload, QemuProcess};
use crate::tests::full_os::{OS_READY_PATTERN, SSH_ARGS, ssh_command};
use crate::util::{NetConfig, allocate_taps, generate_mac};
use anyhow::{Context, Result, bail, ensure};
use log::debug;
use qapi::qmp::{self, RunState};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use test_macro::test_fn;

const GUEST_BIN: &[u8] = include_bytes!("../../payload/guest.bin");
const EXPECTED_OUTPUT: &str = "HELLO FROM GUEST";
const KERNEL: &str = "payload/vmlinuz-virt";
const INITRD: &str = "payload/initrd.img";
const OS_IMAGE: &str = "payload/os-image.qcow2";
const OS_BOOT_TIMEOUT: Duration = Duration::from_secs(60);
const SSH_TIMEOUT: Duration = Duration::from_secs(30);

/// Persistent SSH connection using ControlMaster.
/// The TCP session should survive VM live migration on a bridge,
/// proving that network state migrated correctly.
struct SshSession {
    child: Child,
    control_path: PathBuf,
    key_path: PathBuf,
    host: String,
    user: String,
}

impl SshSession {
    /// Open a ControlMaster connection. Call only after SSH is known to be
    /// reachable (e.g. after a successful `ssh_command`).
    fn open(
        key_path: &Path,
        host: &str,
        port: u16,
        user: &str,
        dir: &Path,
        timeout: Duration,
    ) -> Result<Self> {
        let control_path = dir.join("ssh_ctl");
        let key_str = key_path.to_string_lossy().to_string();
        let ctl_str = format!("ControlPath={}", control_path.display());
        let port_str = port.to_string();
        let user_host = format!("{user}@{host}");
        let var_args: Vec<&str> = vec![
            "-i",
            &key_str,
            "-o",
            "ControlMaster=yes",
            "-o",
            &ctl_str,
            "-o",
            "ControlPersist=yes",
            "-p",
            &port_str,
            "-N",
            &user_host,
        ];
        let mut args = SSH_ARGS.to_vec();
        args.extend_from_slice(&var_args);

        let mut child = Command::new("ssh")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to start SSH ControlMaster")?;

        let start = Instant::now();
        while !control_path.exists() {
            if start.elapsed() > timeout {
                let _ = child.kill();
                let _ = child.wait();
                bail!("SSH ControlMaster socket did not appear within {timeout:?}");
            }
            if let Some(status) = child.try_wait().context("failed to check ssh process")? {
                let stderr = child
                    .stderr
                    .take()
                    .map(|s| std::io::read_to_string(s).unwrap_or_default())
                    .unwrap_or_default();
                bail!("SSH ControlMaster exited early ({status}): {stderr}");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        debug!(
            "SSH ControlMaster established at {}",
            control_path.display()
        );

        Ok(Self {
            child,
            control_path,
            key_path: key_path.to_path_buf(),
            host: host.to_string(),
            user: user.to_string(),
        })
    }

    /// Run a command through the persistent connection.
    fn run(&self, command: &str) -> Result<String> {
        let var_args = [
            "-i",
            &self.key_path.to_string_lossy(),
            "-o",
            &format!("ControlPath={}", self.control_path.display()),
            &format!("{}@{}", self.user, self.host),
            command,
        ];
        let mut args = SSH_ARGS.to_vec();
        args.extend_from_slice(&var_args);

        let output = Command::new("ssh")
            .args(args)
            .output()
            .context("failed to run ssh command via control socket")?;

        ensure!(
            output.status.success(),
            "SSH command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        debug!("ssh (ctl) output: {stdout}");
        Ok(stdout)
    }
}

impl Drop for SshSession {
    fn drop(&mut self) {
        debug!("closing SSH ControlMaster");
        let _ = Command::new("ssh")
            .args([
                "-o",
                &format!("ControlPath={}", self.control_path.display()),
                "-O",
                "exit",
                "dummy",
            ])
            .output();
        let _ = self.child.wait();
    }
}

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
    smp = {1, 2, 4},
    skip = "requires tap networking",
)]
pub(crate) fn test_live_migration_os(machine: Machine, smp: u8) -> Result<()> {
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
        .with_net(src_net);

    // Boot source and wait for login prompt
    let mut src = QemuProcess::spawn(base_cfg.clone()).context("failed to spawn source VM")?;

    let expected = ExpectedOutput::Pattern(OS_READY_PATTERN.try_into()?);
    src.poll_line_timeout(expected, OS_BOOT_TIMEOUT)
        .context("source VM did not boot")?;
    debug!("source VM booted");

    // Verify SSH on source (with retries until guest network is ready)
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

    // Open persistent SSH connection (SSH is known-reachable at this point)
    let session = SshSession::open(
        &ci.ssh_key_path,
        taps.guest_host(),
        22,
        GUEST_USER,
        mig_dir.path(),
        Duration::from_secs(10),
    )
    .context("failed to open SSH ControlMaster")?;

    let kernel_before = session
        .run("uname -r")
        .context("pre-migration SSH check failed")?;
    debug!("source kernel: {kernel_before}");

    // Spawn destination in incoming mode with its own cidata copy
    let dst_cfg = base_cfg
        .with_incoming(&dst_dir)
        .with_cloud_init(dst_cidata_path)
        .with_net(dst_net);
    let mut dst = QemuProcess::spawn(dst_cfg).context("failed to spawn destination VM")?;

    // Migrate
    do_migration(&mut src, &mut dst, &mig_sock)?;
    debug!("migration completed");

    // Drop source to free resources
    drop(src);
    debug!("source VM terminated");

    // Verify the persistent SSH session survived migration
    let kernel_after = session
        .run("uname -r")
        .context("post-migration SSH check failed — connection did not survive")?;
    debug!("destination kernel: {kernel_after}");
    ensure!(
        kernel_before == kernel_after,
        "kernel version mismatch after migration: {kernel_before} vs {kernel_after}"
    );

    Ok(())
}
