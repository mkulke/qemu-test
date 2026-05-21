use crate::output::OutputFormat;
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
    test_repeat: Option<String>,
    keep_logs: Option<String>,
    test_output: Option<String>,
    test_junit_result_path: Option<String>,
}

pub(crate) static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    qemu_bin: env::var("QEMU_BIN").ok(),
    accel: env::var("ACCEL").ok(),
    test_jobs: env::var("TEST_JOBS").ok(),
    test_filter: env::var("TEST_FILTER").ok(),
    test_repeat: env::var("TEST_REPEAT").ok(),
    keep_logs: env::var("KEEP_LOGS").ok(),
    test_output: env::var("TEST_OUTPUT").ok(),
    test_junit_result_path: env::var("TEST_JUNIT_RESULT_PATH").ok(),
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

    pub fn test_output(&self) -> Result<OutputFormat> {
        let Some(value) = self.test_output.as_deref() else {
            return Ok(OutputFormat::default());
        };
        value.try_into()
    }

    pub fn test_junit_result_path(&self) -> &str {
        self.test_junit_result_path.as_deref().unwrap_or("test-results.xml")
    }
}
