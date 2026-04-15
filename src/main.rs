use anyhow::{Result, bail};
use config::CONFIG;
use linkme::distributed_slice;
use rand::seq::SliceRandom;
use rayon::prelude::*;
use std::cell::RefCell;
use util::TestFilter;

mod cloud_init;
mod config;
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

fn run_test(entry: &TestEntry) -> Result<()> {
    let label = entry.0();
    CURRENT_TEST_LABEL.with(|l| *l.borrow_mut() = label.clone());
    println!("TEST: {label}");
    let start = std::time::Instant::now();
    let result = (entry.1)();
    let elapsed = start.elapsed();
    result
        .inspect(|_| println!("PASS: {label} ({:.2}s)", elapsed.as_secs_f64()))
        .inspect_err(|e| println!("FAIL: {label}: {e} ({:.2}s)", elapsed.as_secs_f64()))
}

fn main() -> Result<()> {
    env_logger::init();

    let test_jobs = CONFIG.test_jobs()?;
    let filter: Option<TestFilter> = CONFIG.test_filter()?;

    let mut tests: Vec<&TestEntry> = TESTS
        .iter()
        .filter(|entry| {
            let label = entry.0();
            let Some(filter) = &filter else {
                if let Some(reason) = entry.2 {
                    println!("SKIP: {label} ({reason})");
                    return false;
                }
                return true;
            };
            let matches = filter.matches(&label, entry.2);
            let skipped_by_annotation =
                entry.2.is_some() && filter.matches(&label, None) && !matches;
            if skipped_by_annotation && let Some(reason) = entry.2 {
                println!("SKIP: {label} ({reason})");
            }
            matches
        })
        .collect();

    tests.shuffle(&mut rand::rng());

    if tests.is_empty() {
        bail!("no tests matched filter: {:?}", filter);
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(test_jobs)
        .build()
        .expect("failed to build thread pool");

    let start = std::time::Instant::now();
    let errors: Vec<_> = pool.install(|| {
        tests
            .par_iter()
            .filter_map(|entry| run_test(entry).err())
            .collect()
    });
    let elapsed = start.elapsed();

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("{e:?}");
        }
        bail!(
            "FAIL: {} of {} tests failed ({:.2}s)",
            errors.len(),
            tests.len(),
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
