use crate::config::CONFIG;
use anyhow::{Context, Result};
use log::warn;
use rand::RngExt;
use regex::Regex;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

pub(crate) fn generate_mac() -> String {
    let mut rng = rand::rng();
    let b: [u8; 3] = rng.random();
    format!("52:54:00:{:02x}:{:02x}:{:02x}", b[0], b[1], b[2])
}

const TAP_PREFIX: &str = "tap-qemu-";
const GATEWAY: &str = "192.168.100.1";

static TAP_POOL: Mutex<Option<Vec<usize>>> = Mutex::new(None);
static FILTER_TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9]+(_[a-z0-9]+)*(=[a-z0-9]+(_[a-z0-9]+)*)?$")
        .expect("invalid test filter token regex")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TestFilter {
    include: Vec<String>,
    exclude: Vec<String>,
}

impl TestFilter {
    pub fn parse(raw: &str) -> Result<Self> {
        let mut include = Vec::new();
        let mut exclude = Vec::new();

        for token in raw.split(',') {
            let (target, value) = if let Some(value) = token.strip_prefix('-') {
                (&mut exclude, value)
            } else {
                (&mut include, token)
            };

            if !FILTER_TOKEN_RE.is_match(value) {
                anyhow::bail!(
                    "invalid filter token: '{token}'. expected [a-z0-9]+(_[a-z0-9]+)*(=[a-z0-9]+(_[a-z0-9]+)*)?"
                );
            }
            target.push(value.to_string());
        }

        Ok(Self { include, exclude })
    }

    pub fn matches(&self, label: &str, skip_reason: Option<&str>) -> bool {
        if self.exclude.iter().any(|f| label.contains(f)) {
            return false;
        }

        if !self.include.is_empty() {
            return self.include.iter().any(|f| label.contains(f));
        }

        skip_reason.is_none()
    }
}

fn tap_exists(name: &str) -> bool {
    Path::new(&format!("/sys/class/net/{name}")).exists()
}

fn init_tap_pool() -> Result<Vec<usize>> {
    let jobs = CONFIG.test_jobs()?;
    let valid: Vec<usize> = (0..jobs)
        .filter(|i| {
            let src = format!("{TAP_PREFIX}{}", i * 2);
            let dst = format!("{TAP_PREFIX}{}", i * 2 + 1);
            let ok = tap_exists(&src) && tap_exists(&dst);
            if !ok {
                warn!("tap pair {i} not available ({src}, {dst}), skipping");
            }
            ok
        })
        .collect();
    Ok(valid)
}

/// A pair of tap devices for a migration test. Returns to the pool on drop.
pub(crate) struct TapPair {
    pair_index: usize,
    src_tap: String,
    dst_tap: String,
    guest_ip_cidr: String,
    guest_host: String,
}

impl TapPair {
    fn new(pair_index: usize) -> Self {
        let src_idx = pair_index * 2;
        let dst_idx = src_idx + 1;
        let host_octet = 2 + pair_index;
        Self {
            pair_index,
            src_tap: format!("{TAP_PREFIX}{src_idx}"),
            dst_tap: format!("{TAP_PREFIX}{dst_idx}"),
            guest_ip_cidr: format!("192.168.100.{host_octet}/24"),
            guest_host: format!("192.168.100.{host_octet}"),
        }
    }

    pub fn src(&self) -> &str {
        &self.src_tap
    }

    pub fn dst(&self) -> &str {
        &self.dst_tap
    }

    pub fn guest_ip(&self) -> &str {
        &self.guest_ip_cidr
    }

    pub fn guest_host(&self) -> &str {
        &self.guest_host
    }

    pub fn gateway(&self) -> &str {
        GATEWAY
    }
}

impl Drop for TapPair {
    fn drop(&mut self) {
        let mut pool = TAP_POOL.lock().unwrap();
        if let Some(ref mut indices) = *pool {
            indices.push(self.pair_index);
        }
    }
}

/// Allocate a pair of tap devices from the pool.
pub(crate) fn allocate_taps() -> Result<TapPair> {
    let mut pool = TAP_POOL.lock().unwrap();
    let indices = pool.get_or_insert_with(|| init_tap_pool().expect("failed to init tap pool"));
    let pair_index = indices.pop().context("no tap devices available")?;
    Ok(TapPair::new(pair_index))
}

#[derive(Clone)]
pub(crate) enum NetConfig {
    /// User-net (SLIRP) with SSH port forwarding. SSH via localhost:<discovered port>.
    UserNet { mac: String },
    /// Tap device on a bridge. SSH directly to guest IP.
    Tap {
        mac: String,
        ifname: String,
        guest_ip: String,
        gateway: String,
    },
}

impl NetConfig {
    pub fn user_net() -> Self {
        Self::UserNet {
            mac: generate_mac(),
        }
    }

    pub fn tap(ifname: &str, guest_ip: &str, gateway: &str, mac: &str) -> Self {
        Self::Tap {
            mac: mac.to_string(),
            ifname: ifname.to_string(),
            guest_ip: guest_ip.to_string(),
            gateway: gateway.to_string(),
        }
    }

    pub fn mac(&self) -> &str {
        match self {
            Self::UserNet { mac } | Self::Tap { mac, .. } => mac,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TestFilter;

    #[test]
    fn positive_only_filter_matches_any_positive_token() {
        let filter = TestFilter::parse("migration,simple").expect("filter should parse");
        assert!(filter.matches("migration_os,smp=2", Some("skip")));
        assert!(filter.matches("simple,smp=1", None));
        assert!(!filter.matches("kernel_boot,smp=2", None));
    }

    #[test]
    fn negative_only_filter_respects_skip_annotations() {
        let filter = TestFilter::parse("-migration").expect("filter should parse");
        assert!(!filter.matches("migration_os,smp=2", None));
        assert!(filter.matches("kernel_boot,smp=2", None));
        assert!(!filter.matches("kernel_boot,smp=2", Some("skip reason")));
    }

    #[test]
    fn mixed_filter_matches_positive_and_applies_negative() {
        let filter = TestFilter::parse("-migration,smp=2").expect("filter should parse");
        assert!(filter.matches("kernel_boot,smp=2", Some("skip reason")));
        assert!(!filter.matches("kernel_boot,smp=1", None));
        assert!(!filter.matches("migration_os,smp=2", None));
    }

    #[test]
    fn negative_tokens_override_positive_tokens() {
        let filter = TestFilter::parse("migration,-migration_os").expect("filter should parse");
        assert!(filter.matches("migration_guest", None));
        assert!(!filter.matches("migration_os,smp=2", None));
    }

    #[test]
    fn invalid_tokens_fail_validation() {
        for token in [
            "",
            "-",
            "Migration",
            "smp=2,",
            "--migration",
            "test-case",
            "test.name",
            "smp =2",
        ] {
            assert!(
                TestFilter::parse(token).is_err(),
                "expected invalid: {token}"
            );
        }
    }

    #[test]
    fn valid_tokens_parse() {
        for token in ["kernel_boot", "smp=2", "-migration_os"] {
            assert!(TestFilter::parse(token).is_ok(), "expected valid: {token}");
        }
    }
}
