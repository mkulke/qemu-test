use anyhow::Result;
use config::CONFIG;
use linkme::distributed_slice;
use log::warn;
use output::{
    OutputFormat, TestOutcome, TestResult, has_failures, print_summary, print_test_result,
    print_test_start,
};
use rand::seq::SliceRandom;
use rayon::prelude::*;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use util::TestFilter;

mod cloud_init;
mod config;
mod output;
mod process;
mod tests;
mod util;

// label, test function, skip reason (None = run by default)
pub type TestEntry = (fn() -> String, fn() -> Result<()>, Option<&'static str>);

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

fn run_test(entry: &TestEntry, format: OutputFormat) -> TestResult {
    let label = entry.0();
    CURRENT_TEST_LABEL.with(|l| *l.borrow_mut() = label.clone());

    if SHUTDOWN.load(Ordering::Relaxed) {
        let result = TestResult {
            label,
            outcome: TestOutcome::Fail("interrupted".to_string()),
            duration: std::time::Duration::ZERO,
        };
        print_test_result(format, &result);
        return result;
    }

    print_test_start(format, &label);
    let start = std::time::Instant::now();
    let outcome = match (entry.1)() {
        Ok(()) => TestOutcome::Pass,
        Err(e) => TestOutcome::Fail(format!("{e}")),
    };
    let duration = start.elapsed();
    let result = TestResult {
        label,
        outcome,
        duration,
    };
    print_test_result(format, &result);
    result
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
    let format = CONFIG.test_output()?;

    let mut skip_results: Vec<TestResult> = Vec::new();

    let tests: Vec<&TestEntry> = TESTS
        .iter()
        .filter(|entry| {
            let label = entry.0();
            let Some(filter) = &filter else {
                if let Some(reason) = entry.2 {
                    let result = TestResult {
                        label: label.clone(),
                        outcome: TestOutcome::Skip(reason.to_string()),
                        duration: std::time::Duration::ZERO,
                    };
                    print_test_result(format, &result);
                    skip_results.push(result);
                    return false;
                }
                return true;
            };
            let matches = filter.matches(&label, entry.2);
            let skipped_by_annotation =
                entry.2.is_some() && filter.matches(&label, None) && !matches;
            if skipped_by_annotation && let Some(reason) = entry.2 {
                let result = TestResult {
                    label: label.clone(),
                    outcome: TestOutcome::Skip(reason.to_string()),
                    duration: std::time::Duration::ZERO,
                };
                print_test_result(format, &result);
                skip_results.push(result);
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
    let mut results: Vec<TestResult> = pool.install(|| {
        tests
            .par_iter()
            .map(|entry| run_test(entry, format))
            .collect()
    });
    let elapsed = start.elapsed();

    results.extend(skip_results);
    let failed = has_failures(&results);
    let junit_path = CONFIG.test_junit_result_path();
    print_summary(format, &results, elapsed, junit_path);

    if failed {
        std::process::exit(1);
    }

    Ok(())
}
