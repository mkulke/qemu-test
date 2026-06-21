use crate::process::Accelerator;
use crate::util::TestFilter;
use anyhow::{Context, Result};
use std::env;
use std::sync::LazyLock;
use std::time::Duration;

pub(crate) struct Config {
    qemu_bin: Option<String>,
    accel: Option<String>,
    test_jobs: Option<String>,
    test_filter: Option<String>,
    test_repeat: Option<String>,
    keep_logs: Option<String>,
    test_junit_path: Option<String>,
    test_stress_factor: Option<String>,
    test_migration_stress_timeout_secs: Option<String>,
}

pub(crate) static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    qemu_bin: env::var("QEMU_BIN").ok(),
    accel: env::var("ACCEL").ok(),
    test_jobs: env::var("TEST_JOBS").ok(),
    test_filter: env::var("TEST_FILTER").ok(),
    test_repeat: env::var("TEST_REPEAT").ok(),
    keep_logs: env::var("KEEP_LOGS").ok(),
    test_junit_path: env::var("TEST_JUNIT_PATH").ok(),
    test_stress_factor: env::var("TEST_STRESS_FACTOR").ok(),
    test_migration_stress_timeout_secs: env::var("TEST_MIGRATION_STRESS_TIMEOUT_SECS").ok(),
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

    pub fn test_repeat(&self) -> Result<usize> {
        let Some(value) = self.test_repeat.as_deref() else {
            return Ok(1);
        };
        let repeat: usize = value.parse().context("invalid TEST_REPEAT value")?;
        if repeat == 0 {
            anyhow::bail!("TEST_REPEAT must be at least 1");
        }
        Ok(repeat)
    }

    pub fn test_junit_path(&self) -> Option<&str> {
        self.test_junit_path.as_deref()
    }

    pub fn test_stress_factor(&self) -> Result<f64> {
        let Some(value) = self.test_stress_factor.as_deref() else {
            return Ok(1.0);
        };
        let factor: f64 = value.parse().context("invalid TEST_STRESS_FACTOR value")?;
        if factor <= 0.0 {
            anyhow::bail!("TEST_STRESS_FACTOR must be greater than 0");
        }
        Ok(factor)
    }

    pub fn test_migration_stress_timeout(&self) -> Result<Duration> {
        let Some(value) = self.test_migration_stress_timeout_secs.as_deref() else {
            return Ok(Duration::from_secs(60));
        };
        let secs: u64 = value
            .parse()
            .context("invalid TEST_MIGRATION_STRESS_TIMEOUT_SECS value")?;
        if secs == 0 {
            anyhow::bail!("TEST_MIGRATION_STRESS_TIMEOUT_SECS must be at least 1");
        }
        Ok(Duration::from_secs(secs))
    }
}
