use std::env;
use std::sync::LazyLock;

pub(crate) struct Config {
    qemu_bin: Option<String>,
    accel: Option<String>,
    test_jobs: Option<String>,
}

pub(crate) static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    qemu_bin: env::var("QEMU_BIN").ok(),
    accel: env::var("ACCEL").ok(),
    test_jobs: env::var("TEST_JOBS").ok(),
});

impl Config {
    pub fn qemu_bin(&self) -> Option<&str> {
        self.qemu_bin.as_deref()
    }

    pub fn accel(&self) -> Option<&str> {
        self.accel.as_deref()
    }

    pub fn test_jobs(&self) -> Option<&str> {
        self.test_jobs.as_deref()
    }
}
