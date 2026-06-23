use anyhow::{Context, Result, bail};
use log::debug;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

pub(crate) const SSH_ARGS: [&str; 6] = [
    "-o",
    "StrictHostKeyChecking=no",
    "-o",
    "UserKnownHostsFile=/dev/null",
    "-o",
    "LogLevel=ERROR",
];

pub(crate) fn ssh_command(
    key_path: &Path,
    host: &str,
    port: u16,
    user: &str,
    command: &str,
    timeout: Duration,
) -> Result<String> {
    let start = Instant::now();
    loop {
        if crate::SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("interrupted");
        }
        let var_args = [
            "-o",
            "ConnectTimeout=5",
            "-o",
            "BatchMode=yes",
            "-i",
            &key_path.to_string_lossy(),
            "-p",
            &port.to_string(),
            &format!("{user}@{host}"),
            command,
        ];

        let mut args = SSH_ARGS.to_vec();
        args.extend_from_slice(&var_args);

        let output = Command::new("ssh")
            .args(args)
            .output()
            .context("failed to run ssh")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            debug!("ssh output: {stdout}");
            return Ok(stdout);
        }

        let code = output.status.code();
        let stderr = String::from_utf8_lossy(&output.stderr);

        // ssh uses exit code 255 for its own connection/protocol errors; any
        // other non-zero exit code is the remote command's exit status and
        // should not be retried.
        if code != Some(255) {
            let stdout = String::from_utf8_lossy(&output.stdout);
            bail!(
                "remote command failed (exit {:?}): stderr={} stdout={}",
                code,
                stderr.trim(),
                stdout.trim()
            );
        }

        if start.elapsed() > timeout {
            bail!("SSH failed after {timeout:?}: {}", stderr.trim());
        }

        debug!("SSH not ready ({}), retrying...", stderr.trim());
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Send an SSH command without waiting for a response or retrying.
/// Used for commands like `sudo reboot` where the connection will drop.
pub(crate) fn ssh_fire_and_forget(
    key_path: &Path,
    host: &str,
    port: u16,
    user: &str,
    command: &str,
) {
    let var_args = [
        "-o",
        "ConnectTimeout=5",
        "-o",
        "BatchMode=yes",
        "-i",
        &key_path.to_string_lossy(),
        "-p",
        &port.to_string(),
        &format!("{user}@{host}"),
        command,
    ];

    let mut args = SSH_ARGS.to_vec();
    args.extend_from_slice(&var_args);

    match Command::new("ssh").args(args).output() {
        Ok(output) => debug!("fire-and-forget ssh exited with {}", output.status),
        Err(e) => debug!("fire-and-forget ssh failed: {e}"),
    }
}
