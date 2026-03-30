use anyhow::{Context, Result, bail};
use config::CONFIG;
use linkme::distributed_slice;
use rayon::prelude::*;

mod config;
mod process;
mod tests;

#[distributed_slice]
pub static TESTS: [fn() -> Result<()>];

const DEFAULT_TEST_JOBS: usize = 1;

fn main() -> Result<()> {
    env_logger::init();

    let test_jobs = match CONFIG.test_jobs() {
        Some(v) => v.parse().context("invalid TEST_JOBS value")?,
        None => DEFAULT_TEST_JOBS,
    };

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(test_jobs)
        .build()
        .expect("failed to build thread pool");

    let start = std::time::Instant::now();
    let errors: Vec<_> =
        pool.install(|| TESTS.par_iter().filter_map(|test| test().err()).collect());
    let elapsed = start.elapsed();

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("{e:?}");
        }
        bail!(
            "FAIL: {} of {} tests failed ({:.2}s)",
            errors.len(),
            TESTS.len(),
            elapsed.as_secs_f64()
        );
    }

    println!(
        "\nPASS: All {} tests passed ({:.2}s)",
        TESTS.len(),
        elapsed.as_secs_f64()
    );
    Ok(())
}
