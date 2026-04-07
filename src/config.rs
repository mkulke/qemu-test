use crate::process::Accelerator;
use anyhow::{Context, Result};
use std::env;
use std::sync::LazyLock;

pub(crate) struct Config {
    qemu_bin: Option<String>,
    accel: Option<String>,
    test_jobs: Option<String>,
    test_filter: Option<String>,
}

pub(crate) static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    qemu_bin: env::var("QEMU_BIN").ok(),
    accel: env::var("ACCEL").ok(),
    test_jobs: env::var("TEST_JOBS").ok(),
    test_filter: env::var("TEST_FILTER").ok(),
});

const DEFAULT_ACCELERATOR: Accelerator = Accelerator::Kvm;

impl Config {
    pub fn qemu_bin(&self) -> Option<&str> {
        self.qemu_bin.as_deref()
    }

    pub fn accel(&self) -> Result<Accelerator> {
        let Some(value) = self.accel.as_deref() else {
            return Ok(DEFAULT_ACCELERATOR);
        };
        let accel = value
            .try_into()
            .context(format!("invalid accelerator: {}", value))?;
        Ok(accel)
    }

    pub fn test_jobs(&self) -> Option<&str> {
        self.test_jobs.as_deref()
    }

    pub fn test_filter(&self) -> Option<&str> {
        self.test_filter.as_deref()
    }
}
