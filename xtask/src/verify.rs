//! Running the test suite and saying what did not run.
//!
//! `cargo test` captures the output of a passing test, so a test that decides
//! it cannot run, prints why, and returns says nothing at all: the run reports
//! it as passed and the reason is discarded. Every skip in this workspace works
//! that way, because the alternative — failing when a device lacks a capability
//! — would make the suite unrunnable on the machines the suite exists to cover.
//!
//! That is a reasonable design with one hole in it, which is that nobody sees
//! the skips. This runs the suite with output uncaptured and counts them, so a
//! run reports what it covered rather than only that it was green. It found
//! three tests that had been skipping since they were written, and a whole file
//! of them skipping because of a lock two threads were contending for.
//!
//! A skip is not a failure and this does not treat it as one. Some are correct:
//! a device without the advanced-blend extension genuinely cannot render those
//! scenes, and the corpus reports that as coverage it did not get. The point is
//! that the number is visible and has to be looked at.

use std::process::Command;

/// One kind of skip, and how many tests reported it.
pub struct Skip {
    pub reason: String,
    pub count: usize,
}

pub struct Outcome {
    pub passed: usize,
    pub failed: usize,
    pub skips: Vec<Skip>,
    /// True where the suite itself came back non-zero.
    pub broke: bool,
}

/// Everything after the last `(` in a skip line, dropped.
///
/// Reasons carry a device name or an error in parentheses, which differs per
/// machine and would make the same skip count as several. The prefix is what
/// identifies it.
fn normalize(line: &str) -> String {
    let line = line.trim();
    match line.find('(') {
        Some(i) => line[..i].trim_end().to_string(),
        None => line.to_string(),
    }
}

pub fn run(extra: &[String]) -> Outcome {
    let mut command = Command::new(env!("CARGO"));
    command.arg("test").arg("--workspace");
    // Every target runs even after one fails, because this is a census: a run
    // that stops at the first failing binary reports the skips of the targets
    // it reached and says nothing about the rest, which is the same partial
    // picture this command exists to replace.
    command.arg("--no-fail-fast");
    for arg in extra {
        command.arg(arg);
    }
    // Uncaptured, which is the whole point: without this the skip lines below
    // never reach the output at all.
    command.arg("--").arg("--nocapture");

    let output = match command.output() {
        Ok(output) => output,
        Err(e) => {
            eprintln!("could not run the suite: {e}");
            return Outcome {
                passed: 0,
                failed: 0,
                skips: Vec::new(),
                broke: true,
            };
        }
    };

    // Both streams: the harness writes its summary lines to stdout and a test's
    // own output to stderr, and a skip is the latter.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut passed = 0;
    let mut failed = 0;
    let mut reasons: Vec<(String, usize)> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("test result: ") {
            // "ok. 12 passed; 0 failed; ..."
            let mut fields = rest.split_whitespace();
            let _verdict = fields.next();
            if let (Some(n), Some("passed;")) = (fields.next(), fields.next()) {
                passed += n.parse::<usize>().unwrap_or(0);
            }
            if let (Some(n), Some("failed;")) = (fields.next(), fields.next()) {
                failed += n.parse::<usize>().unwrap_or(0);
            }
        }
        if line.contains("skipping") {
            let reason = normalize(line);
            match reasons.iter_mut().find(|(r, _)| *r == reason) {
                Some((_, count)) => *count += 1,
                None => reasons.push((reason, 1)),
            }
        }
    }
    reasons.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    Outcome {
        passed,
        failed,
        skips: reasons
            .into_iter()
            .map(|(reason, count)| Skip { reason, count })
            .collect(),
        broke: !output.status.success(),
    }
}

pub fn text(outcome: &Outcome) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} passed, {} failed\n",
        outcome.passed, outcome.failed
    ));
    let skipped: usize = outcome.skips.iter().map(|s| s.count).sum();
    if outcome.skips.is_empty() {
        out.push_str("nothing skipped\n");
    } else {
        out.push_str(&format!("{skipped} skipped:\n"));
        for skip in &outcome.skips {
            out.push_str(&format!("  {:>3}  {}\n", skip.count, skip.reason));
        }
        out.push_str(
            "\nA skip passes, so these are tests that did not run. Some are \n\
             correct -- a device without a capability cannot exercise it -- and \n\
             the ones that are not look exactly the same from here.\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reason_is_identified_by_its_prefix() {
        // The parenthesized part names a device or an error, which differs per
        // machine; counting it would split one skip into several.
        assert_eq!(
            normalize("skipping: no Vulkan device (ERROR_INITIALIZATION_FAILED)"),
            "skipping: no Vulkan device"
        );
        assert_eq!(
            normalize("  skipping: both backends are needed"),
            "skipping: both backends are needed"
        );
    }
}
