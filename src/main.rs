use anyhow::{Result, bail};
use config::CONFIG;
use junit::{TestOutcome, TestResult, write_junit};
use linkme::distributed_slice;
use log::warn;
use rand::seq::SliceRandom;
use rayon::prelude::*;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use util::TestFilter;

mod cloud_init;
mod config;
mod junit;
mod process;
mod ssh;
mod tests;
mod util;

// name, properties, test function, skip reason (None = run by default)
pub struct TestEntry {
    pub name: &'static str,
    pub props_fn: fn() -> Vec<(&'static str, String)>,
    pub test_fn: fn() -> Result<()>,
    pub skip: Option<&'static str>,
}

impl TestEntry {
    pub fn label(&self) -> String {
        let props = (self.props_fn)();
        if props.is_empty() {
            self.name.to_string()
        } else {
            let params: Vec<String> = props.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!("{}({})", self.name, params.join(", "))
        }
    }

    pub fn properties(&self) -> Vec<(&'static str, String)> {
        (self.props_fn)()
    }
}

#[distributed_slice]
pub static TESTS: [TestEntry];

thread_local! {
    pub static CURRENT_TEST_LABEL: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Set by the SIGINT handler to request graceful shutdown.
pub static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn sigint_handler(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::Relaxed);
}

fn run_test(entry: &TestEntry) -> TestResult {
    let label = entry.label();
    let properties = entry.properties();
    CURRENT_TEST_LABEL.with(|l| *l.borrow_mut() = label.clone());

    if SHUTDOWN.load(Ordering::Relaxed) {
        println!("FAIL: {label}: interrupted (0.00s)");
        return TestResult {
            label,
            properties,
            outcome: TestOutcome::Fail("interrupted".to_string()),
            duration: std::time::Duration::ZERO,
        };
    }

    println!("TEST: {label}");
    let start = std::time::Instant::now();
    let result = (entry.test_fn)();
    let duration = start.elapsed();

    let outcome = match result {
        Ok(()) => {
            println!("PASS: {label} ({:.2}s)", duration.as_secs_f64());
            TestOutcome::Pass
        }
        Err(e) => {
            let msg = format!("{e}");
            println!("FAIL: {label}: {msg} ({:.2}s)", duration.as_secs_f64());
            TestOutcome::Fail(msg)
        }
    };

    TestResult {
        label,
        properties,
        outcome,
        duration,
    }
}

fn main() -> Result<()> {
    env_logger::init();

    // Install SIGINT handler for graceful shutdown with proper cleanup.
    unsafe {
        libc::signal(
            libc::SIGINT,
            sigint_handler as *const () as libc::sighandler_t,
        );
    }

    let test_jobs = CONFIG.test_jobs()?;
    let test_repeat = CONFIG.test_repeat()?;
    let filter: Option<TestFilter> = CONFIG.test_filter()?;
    let junit_path = CONFIG.test_junit_path();

    let mut skip_results: Vec<TestResult> = Vec::new();

    let tests: Vec<&TestEntry> = TESTS
        .iter()
        .filter(|entry| {
            let label = entry.label();
            let Some(filter) = &filter else {
                if let Some(reason) = entry.skip {
                    println!("SKIP: {label} ({reason})");
                    if junit_path.is_some() {
                        skip_results.push(TestResult {
                            label: label.clone(),
                            properties: entry.properties(),
                            outcome: TestOutcome::Skip(reason.to_string()),
                            duration: std::time::Duration::ZERO,
                        });
                    }
                    return false;
                }
                return true;
            };
            let matches = filter.matches(&label, entry.skip);
            let skipped_by_annotation =
                entry.skip.is_some() && filter.matches(&label, None) && !matches;
            if skipped_by_annotation && let Some(reason) = entry.skip {
                println!("SKIP: {label} ({reason})");
                if junit_path.is_some() {
                    skip_results.push(TestResult {
                        label: label.clone(),
                        properties: entry.properties(),
                        outcome: TestOutcome::Skip(reason.to_string()),
                        duration: std::time::Duration::ZERO,
                    });
                }
            }
            matches
        })
        .collect();

    let mut tests = tests.repeat(test_repeat);
    tests.shuffle(&mut rand::rng());

    if let Some(filter) = filter
        && tests.is_empty()
    {
        warn!("no tests matched provided filter ({})", filter);
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(test_jobs)
        .build()
        .expect("failed to build thread pool");

    let start = std::time::Instant::now();
    let results: Vec<TestResult> =
        pool.install(|| tests.par_iter().map(|entry| run_test(entry)).collect());
    let elapsed = start.elapsed();

    let failures: Vec<_> = results
        .iter()
        .filter_map(|r| match &r.outcome {
            TestOutcome::Fail(e) => Some((r.label.clone(), e.clone())),
            _ => None,
        })
        .collect();
    let num_run = results.len();

    if let Some(path) = junit_path {
        let mut all_results = skip_results;
        all_results.extend(results);
        write_junit(path, &all_results, elapsed);
    }

    if !failures.is_empty() {
        eprintln!();
        for (label, e) in &failures {
            eprintln!("FAIL: {label}: {e}");
        }
        bail!(
            "\n{} of {} tests failed ({:.2}s)",
            failures.len(),
            num_run,
            elapsed.as_secs_f64()
        );
    }

    println!(
        "\nPASS: All {} tests passed ({:.2}s)",
        tests.len(),
        elapsed.as_secs_f64()
    );
    Ok(())
}
