use crate::process::Accelerator;
use crate::util::TestFilter;
use anyhow::{Context, Result};
use std::env;
use std::sync::LazyLock;

pub(crate) struct Config {
    qemu_bin: Option<String>,
    accel: Option<String>,
    test_jobs: Option<String>,
    test_filter: Option<String>,
    keep_logs: Option<String>,
}

pub(crate) static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    qemu_bin: env::var("QEMU_BIN").ok(),
    accel: env::var("ACCEL").ok(),
    test_jobs: env::var("TEST_JOBS").ok(),
    test_filter: env::var("TEST_FILTER").ok(),
    keep_logs: env::var("KEEP_LOGS").ok(),
});

const DEFAULT_ACCELERATOR: Accelerator = Accelerator::Kvm;
const DEFAULT_TEST_JOBS: usize = 1;

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

    pub fn test_jobs(&self) -> Result<usize> {
        let Some(value) = self.test_jobs.as_deref() else {
            return Ok(DEFAULT_TEST_JOBS);
        };

        let jobs = value.parse().context("invalid TEST_JOBS value")?;
        Ok(jobs)
    }

    pub fn test_filter(&self) -> Result<Option<TestFilter>> {
        self.test_filter
            .as_deref()
            .map(TestFilter::parse)
            .transpose()
    }

    pub fn keep_logs(&self) -> Option<&str> {
        self.keep_logs.as_deref()
    }
}
