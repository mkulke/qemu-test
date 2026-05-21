use std::fmt::Write;
use std::io::Write as IoWrite;
use std::time::Duration;

use anyhow::{Result, bail};

const JUNIT_OUTPUT_FILE: &str = "test-results.xml";

#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum OutputFormat {
    #[default]
    Text,
    Dot,
    Junit,
}

impl TryFrom<&str> for OutputFormat {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "text" => Ok(Self::Text),
            "dot" => Ok(Self::Dot),
            "junit" => Ok(Self::Junit),
            _ => bail!("invalid TEST_OUTPUT value: '{}' (expected text|dot|junit)", value),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TestResult {
    pub label: String,
    pub outcome: TestOutcome,
    pub duration: Duration,
}

#[derive(Debug, Clone)]
pub(crate) enum TestOutcome {
    Pass,
    Fail(String),
    Skip(String),
}

pub(crate) fn print_test_start(format: OutputFormat, label: &str) {
    match format {
        OutputFormat::Text => println!("TEST: {label}"),
        OutputFormat::Dot | OutputFormat::Junit => {}
    }
}

pub(crate) fn print_test_result(format: OutputFormat, result: &TestResult) {
    match format {
        OutputFormat::Text => match &result.outcome {
            TestOutcome::Pass => {
                println!("PASS: {} ({:.2}s)", result.label, result.duration.as_secs_f64())
            }
            TestOutcome::Fail(e) => {
                println!(
                    "FAIL: {}: {} ({:.2}s)",
                    result.label,
                    e,
                    result.duration.as_secs_f64()
                )
            }
            TestOutcome::Skip(reason) => {
                println!("SKIP: {} ({})", result.label, reason)
            }
        },
        OutputFormat::Dot => match &result.outcome {
            TestOutcome::Pass => { print!("."); let _ = std::io::stdout().flush(); }
            TestOutcome::Fail(_) => { print!("F"); let _ = std::io::stdout().flush(); }
            TestOutcome::Skip(_) => {}
        },
        OutputFormat::Junit => {}
    }
}

pub(crate) fn print_summary(
    format: OutputFormat,
    results: &[TestResult],
    total_duration: Duration,
) {
    match format {
        OutputFormat::Text => {
            let failures: Vec<_> = results
                .iter()
                .filter(|r| matches!(r.outcome, TestOutcome::Fail(_)))
                .collect();
            if failures.is_empty() {
                println!(
                    "\nPASS: All {} tests passed ({:.2}s)",
                    results.len(),
                    total_duration.as_secs_f64()
                );
            } else {
                eprintln!();
                for r in &failures {
                    if let TestOutcome::Fail(e) = &r.outcome {
                        eprintln!("FAIL: {}: {e}", r.label);
                    }
                }
                eprintln!(
                    "\n{} of {} tests failed ({:.2}s)",
                    failures.len(),
                    results.len(),
                    total_duration.as_secs_f64()
                );
            }
        }
        OutputFormat::Dot => {
            println!();
            let failures: Vec<_> = results
                .iter()
                .filter(|r| matches!(r.outcome, TestOutcome::Fail(_)))
                .collect();
            let skipped = results
                .iter()
                .filter(|r| matches!(r.outcome, TestOutcome::Skip(_)))
                .count();
            let run = results.len() - skipped;
            if failures.is_empty() {
                println!(
                    "PASS: {run} tests passed, {skipped} skipped ({:.2}s)",
                    total_duration.as_secs_f64()
                );
            } else {
                println!();
                for r in &failures {
                    if let TestOutcome::Fail(e) = &r.outcome {
                        eprintln!("FAIL: {}: {e}", r.label);
                    }
                }
                eprintln!(
                    "{} of {run} tests failed, {skipped} skipped ({:.2}s)",
                    failures.len(),
                    total_duration.as_secs_f64()
                );
            }
        }
        OutputFormat::Junit => {
            let xml = format_junit(results, total_duration);
            std::fs::write(JUNIT_OUTPUT_FILE, &xml)
                .expect("failed to write JUnit report");
            println!("JUnit report written to {JUNIT_OUTPUT_FILE}");
        }
    }
}

pub(crate) fn has_failures(results: &[TestResult]) -> bool {
    results
        .iter()
        .any(|r| matches!(r.outcome, TestOutcome::Fail(_)))
}

fn format_junit(results: &[TestResult], total_duration: Duration) -> String {
    let total = results.len();
    let failures = results
        .iter()
        .filter(|r| matches!(r.outcome, TestOutcome::Fail(_)))
        .count();
    let skipped = results
        .iter()
        .filter(|r| matches!(r.outcome, TestOutcome::Skip(_)))
        .count();

    let mut xml = String::new();
    writeln!(xml, r#"<?xml version="1.0" encoding="UTF-8"?>"#).unwrap();
    writeln!(
        xml,
        r#"<testsuites tests="{total}" failures="{failures}" skipped="{skipped}" time="{:.3}">"#,
        total_duration.as_secs_f64()
    )
    .unwrap();
    writeln!(
        xml,
        r#"  <testsuite name="qemu-test" tests="{total}" failures="{failures}" skipped="{skipped}" time="{:.3}">"#,
        total_duration.as_secs_f64()
    )
    .unwrap();

    for result in results {
        let name = xml_escape(&result.label);
        let time = format!("{:.3}", result.duration.as_secs_f64());
        match &result.outcome {
            TestOutcome::Pass => {
                writeln!(
                    xml,
                    r#"    <testcase name="{name}" classname="qemu-test" time="{time}" />"#
                )
                .unwrap();
            }
            TestOutcome::Fail(msg) => {
                writeln!(
                    xml,
                    r#"    <testcase name="{name}" classname="qemu-test" time="{time}">"#
                )
                .unwrap();
                writeln!(
                    xml,
                    r#"      <failure message="{}">{}</failure>"#,
                    xml_escape(msg),
                    xml_escape(msg)
                )
                .unwrap();
                writeln!(xml, r#"    </testcase>"#).unwrap();
            }
            TestOutcome::Skip(reason) => {
                writeln!(
                    xml,
                    r#"    <testcase name="{name}" classname="qemu-test" time="{time}">"#
                )
                .unwrap();
                writeln!(
                    xml,
                    r#"      <skipped message="{}" />"#,
                    xml_escape(reason)
                )
                .unwrap();
                writeln!(xml, r#"    </testcase>"#).unwrap();
            }
        }
    }

    writeln!(xml, r#"  </testsuite>"#).unwrap();
    writeln!(xml, r#"</testsuites>"#).unwrap();
    xml
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
