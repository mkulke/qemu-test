use std::fmt::Write;
use std::time::Duration;

pub(crate) struct TestResult {
    pub label: String,
    pub properties: Vec<(&'static str, String)>,
    pub outcome: TestOutcome,
    pub duration: Duration,
}

pub(crate) enum TestOutcome {
    Pass,
    Fail(String),
    Skip(String),
}

pub(crate) fn write_junit(path: &str, results: &[TestResult], total_duration: Duration) {
    let xml = format_junit(results, total_duration);
    std::fs::write(path, &xml).expect("failed to write JUnit report");
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
            TestOutcome::Pass if result.properties.is_empty() => {
                writeln!(
                    xml,
                    r#"    <testcase name="{name}" classname="qemu-test" time="{time}" />"#
                )
                .unwrap();
            }
            outcome => {
                writeln!(
                    xml,
                    r#"    <testcase name="{name}" classname="qemu-test" time="{time}">"#
                )
                .unwrap();
                if !result.properties.is_empty() {
                    writeln!(xml, r#"      <properties>"#).unwrap();
                    for (key, value) in &result.properties {
                        writeln!(
                            xml,
                            r#"        <property name="{}" value="{}" />"#,
                            xml_escape(key),
                            xml_escape(value)
                        )
                        .unwrap();
                    }
                    writeln!(xml, r#"      </properties>"#).unwrap();
                }
                match outcome {
                    TestOutcome::Fail(msg) => {
                        writeln!(
                            xml,
                            r#"      <failure message="{}">{}</failure>"#,
                            xml_escape(msg),
                            xml_escape(msg)
                        )
                        .unwrap();
                    }
                    TestOutcome::Skip(reason) => {
                        writeln!(xml, r#"      <skipped message="{}" />"#, xml_escape(reason))
                            .unwrap();
                    }
                    _ => {}
                }
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
