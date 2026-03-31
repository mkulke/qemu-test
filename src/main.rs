use anyhow::{Context, Result, bail};
use config::CONFIG;
use linkme::distributed_slice;
use rayon::prelude::*;

mod config;
mod process;
mod tests;

// label, test function
pub type TestEntry = (fn() -> String, fn() -> Result<()>);

#[distributed_slice]
pub static TESTS: [TestEntry];

const DEFAULT_TEST_JOBS: usize = 1;

fn run_test(entry: &TestEntry) -> Result<()> {
    let label = entry.0();
    println!("TEST: {label}");
    let start = std::time::Instant::now();
    let result = (entry.1)();
    let elapsed = start.elapsed();
    result
        .inspect(|_| println!("PASS: {label} ({:.2}s)", elapsed.as_secs_f64()))
        .inspect_err(|e| eprintln!("FAIL: {label}: {e} ({:.2}s)", elapsed.as_secs_f64()))
}

fn main() -> Result<()> {
    env_logger::init();

    let test_jobs = match CONFIG.test_jobs() {
        Some(v) => v.parse().context("invalid TEST_JOBS value")?,
        None => DEFAULT_TEST_JOBS,
    };

    let filters: Vec<&str> = CONFIG
        .test_filter()
        .map(|f| f.split(',').collect())
        .unwrap_or_default();

    let tests: Vec<&TestEntry> = TESTS
        .iter()
        .filter(|entry| {
            if filters.is_empty() {
                return true;
            }
            let label = entry.0();
            filters.iter().any(|f| label.contains(f))
        })
        .collect();

    if tests.is_empty() {
        bail!("no tests matched filter: {:?}", filters);
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
