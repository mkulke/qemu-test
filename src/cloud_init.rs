use crate::util::NetConfig;
use anyhow::{Context, Result};
use indoc::formatdoc;
use log::debug;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const GUEST_USER: &str = "cloud";

pub(crate) struct CloudInitDisk {
    pub path: PathBuf,
    pub ssh_key_path: PathBuf,
}

fn build_network_config(net: &NetConfig) -> String {
    match net {
        NetConfig::UserNet { mac } => formatdoc! {"
            version: 2
            ethernets:
              eth0:
                match:
                  macaddress: \"{mac}\"
                dhcp4: true
        "},
        NetConfig::Tap {
            guest_ip,
            gateway,
            mac,
            ..
        } => formatdoc! {"
            version: 2
            ethernets:
              eth0:
                match:
                  macaddress: \"{mac}\"
                addresses:
                - \"{guest_ip}\"
                routes:
                - to: \"default\"
                  via: \"{gateway}\"
        "},
    }
}

fn create_cidata_disk(path: &Path, public_key: &str, net: &NetConfig) -> Result<()> {
    let dir = path.parent().context("no parent dir")?;
    let cidata_dir = dir.join("cidata");
    fs::create_dir_all(&cidata_dir)?;

    let meta_data = formatdoc! {"
        instance-id: {GUEST_USER}
        local-hostname: {GUEST_USER}
    "};

    let network_config = build_network_config(net);

    let user_data = formatdoc! {"
        #cloud-config
        users:
        - name: {GUEST_USER}
          sudo: ALL=(ALL) NOPASSWD:ALL
          lock_passwd: false
          ssh_authorized_keys:
          - {public_key}
        ssh_pwauth: true
    "};

    for (name, content) in [
        ("meta-data", meta_data.as_str()),
        ("user-data", user_data.as_str()),
        ("network-config", network_config.as_str()),
    ] {
        fs::write(cidata_dir.join(name), content)
            .with_context(|| format!("failed to write {name}"))?;
    }

    let path_str = path.to_string_lossy();
    run_cmd("mkdosfs", &["-n", "CIDATA", "-C", &path_str, "8192"])?;

    for name in ["meta-data", "user-data", "network-config"] {
        let src = cidata_dir.join(name).to_string_lossy().to_string();
        run_cmd("mcopy", &["-oi", &path_str, "-s", &src, "::"])?;
    }

    Ok(())
}

impl CloudInitDisk {
    pub fn create(dir: &Path, net: &NetConfig) -> Result<Self> {
        let ssh_key_path = dir.join("id_cloud");
        let status = Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-f",
                &ssh_key_path.to_string_lossy(),
                "-N",
                "",
                "-q",
            ])
            .status()
            .context("failed to run ssh-keygen")?;
        anyhow::ensure!(status.success(), "ssh-keygen failed");

        let public_key = fs::read_to_string(format!("{}.pub", ssh_key_path.display()))
            .context("failed to read public key")?;
        let public_key = public_key.trim();
        debug!("generated SSH key: {}", ssh_key_path.display());

        let disk_path = dir.join("cidata.img");
        create_cidata_disk(&disk_path, public_key, net)?;
        debug!(
            "wrote cloud-init disk to {} (mac={})",
            disk_path.display(),
            net.mac()
        );

        Ok(Self {
            path: disk_path,
            ssh_key_path,
        })
    }
}

fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    anyhow::ensure!(
        status.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    Ok(())
}
