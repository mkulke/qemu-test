use crate::util::NetConfig;
use anyhow::{Context, Result};
use base64::prelude::*;
use indoc::formatdoc;
use log::debug;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const GUEST_USER: &str = "cloud";

#[derive(Default)]
pub(crate) struct CloudInitDisk {
    path: PathBuf,
    ssh_key_path: PathBuf,
    net_config: Option<NetConfig>,
    write_files: Vec<(String, String)>,
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
                - to: \"0.0.0.0/0\"
                  via: \"{gateway}\"
        "},
    }
}

impl CloudInitDisk {
    pub fn new(dir: &Path) -> Result<Self> {
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
        debug!("generated SSH key: {}", ssh_key_path.display());

        let path = dir.join("cidata.img");

        Ok(Self {
            path,
            ssh_key_path,
            ..Default::default()
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn ssh_key_path(&self) -> &Path {
        &self.ssh_key_path
    }

    pub fn with_net_config(mut self, net_config: &NetConfig) -> Self {
        self.net_config = Some(net_config.clone());
        self
    }

    pub fn with_write_files(mut self, files: &[(&str, &str)]) -> Self {
        for (path, content) in files {
            let file: (String, String) = (path.to_string(), content.to_string());
            self.write_files.push(file);
        }
        self
    }

    pub fn create(&mut self) -> Result<()> {
        let public_key = fs::read_to_string(format!("{}.pub", self.ssh_key_path.display()))
            .context("failed to read public key")?;
        let public_key = public_key.trim();
        debug!("generated SSH key: {}", self.ssh_key_path.display());

        let dir = self.path.parent().context("no parent dir")?;
        let cidata_dir = dir.join("cidata");
        fs::create_dir_all(&cidata_dir)?;

        let meta_data = formatdoc! {"
            instance-id: {GUEST_USER}
            local-hostname: {GUEST_USER}
        "};

        let mut user_data = formatdoc! {"
            #cloud-config
            users:
            - name: {GUEST_USER}
              sudo: ALL=(ALL) NOPASSWD:ALL
              lock_passwd: false
              ssh_authorized_keys:
              - {public_key}
            ssh_pwauth: true
        "};

        if !self.write_files.is_empty() {
            user_data.push_str("\nwrite_files:\n");
            for (path, content) in self.write_files.iter() {
                debug!("adding write_files entry {}", path);
                let b64_content = BASE64_STANDARD.encode(content);
                let entry = formatdoc! {"
                - path: {path}
                  permissions: '0644'
                  encoding: b64
                  content: {b64_content}
                "};
                user_data.push_str(&entry);
            }
        }

        let mut files = vec![("meta-data", meta_data), ("user-data", user_data)];

        if let Some(net) = self.net_config.as_ref() {
            debug!("using network config: {:?}", net);
            let network_config = build_network_config(net);
            files.push(("network-config", network_config));
        }

        for (name, content) in files.iter() {
            fs::write(cidata_dir.join(name), content)
                .with_context(|| format!("failed to write {name}"))?;
        }

        let path_str = self.path.to_string_lossy();
        run_cmd("mkdosfs", &["-n", "CIDATA", "-C", &path_str, "8192"])?;

        for (name, _) in files.iter() {
            let src = cidata_dir.join(name).to_string_lossy().to_string();
            run_cmd("mcopy", &["-oi", &path_str, "-s", &src, "::"])?;
        }

        debug!("wrote cloud-init disk to {}", self.path.display());

        Ok(())
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
