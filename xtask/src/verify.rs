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

/// A failed test and the panic it produced, if one was found.
pub struct Failure {
    pub name: String,
    /// The `thread ... panicked at` location, and the message under it.
    ///
    /// Empty when the failure was not a panic -- a test binary that aborted,
    /// or a harness-level error -- which is itself worth seeing rather than
    /// papering over with a guess.
    pub detail: Vec<String>,
}

/// One kind of skip, and how many tests reported it.
pub struct Skip {
    pub reason: String,
    pub count: usize,
}

pub struct Outcome {
    pub passed: usize,
    pub failed: usize,
    /// The tests that failed, each with whatever it said on the way out.
    ///
    /// A count on its own says something is wrong and not what, and a name on
    /// its own says which test and not why. On a machine that is not this one
    /// -- a CI runner, somebody else's hardware -- the difference between those
    /// and the panic message is the difference between a fix and another push
    /// to find out. Both were learned that way.
    pub failures: Vec<Failure>,
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
                failures: Vec::new(),
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

    parse(&text, !output.status.success())
}

/// Turn the harness's output into a census.
///
/// Split from the run so it can be tested on text rather than on a device: the
/// interesting cases here are shapes of output -- a panic with a multi-line
/// message, the same failure named once per target under `--no-fail-fast` --
/// and reproducing those by breaking a real test is neither quick nor
/// something that stays around afterwards.
fn parse(text: &str, broke: bool) -> Outcome {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<Failure> = Vec::new();
    // Collected separately and joined afterwards, because the two are not in
    // the order they read in. Under `--nocapture` a panic is written when it
    // happens and the harness prints `... FAILED` when the test finishes, so
    // the message arrives first -- and with tests running in parallel, other
    // tests' output arrives in between. Matching them as they were scanned
    // found nothing at all, and did so only on the machine that had a failure
    // to report.
    let mut panics: Vec<(String, Vec<String>)> = Vec::new();
    let mut reasons: Vec<(String, usize)> = Vec::new();
    // The harness prints each failure twice: once as it happens and again in a
    // trailing list. The first form is taken and the second ignored, because
    // with `--no-fail-fast` the trailing lists arrive per target and a name
    // would otherwise be counted once per binary that mentions it.
    //
    // The panic is matched separately, by the thread name the harness gives
    // each test. Under `--nocapture` it is written straight to stderr as it
    // happens rather than collected into a per-test block, so the two are
    // interleaved with everything else and are paired up by name here.
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if let Some(name) = line.trim().strip_prefix("test ") {
            if let Some(name) = name.strip_suffix(" ... FAILED") {
                let name = name.to_string();
                if !failures.iter().any(|f| f.name == name) {
                    failures.push(Failure {
                        name,
                        detail: Vec::new(),
                    });
                }
            }
        }
        if let Some(rest) = line.trim().strip_prefix("thread '") {
            if let Some((thread, location)) = rest.split_once("' panicked at ") {
                if !panics.iter().any(|(name, _)| name == thread) {
                    let mut detail = vec![format!("at {location}")];
                    // The message sits under the location, one line or
                    // several, and ends where the harness's note about
                    // RUST_BACKTRACE begins.
                    for next in lines.iter().skip(i + 1) {
                        let next = next.trim();
                        if next.is_empty() || next.starts_with("note: ") {
                            break;
                        }
                        detail.push(next.to_string());
                    }
                    panics.push((thread.to_string(), detail));
                }
            }
        }
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
    for failure in &mut failures {
        if let Some((_, detail)) = panics.iter().find(|(name, _)| *name == failure.name) {
            failure.detail = detail.clone();
        }
    }
    reasons.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    Outcome {
        passed,
        failed,
        failures,
        skips: reasons
            .into_iter()
            .map(|(reason, count)| Skip { reason, count })
            .collect(),
        broke,
    }
}

pub fn text(outcome: &Outcome) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} passed, {} failed\n",
        outcome.passed, outcome.failed
    ));
    for failure in &outcome.failures {
        out.push_str(&format!("    FAILED  {}\n", failure.name));
        for line in &failure.detail {
            out.push_str(&format!("              {line}\n"));
        }
    }
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

    /// The shape `cargo test --no-fail-fast -- --nocapture` actually produces.
    ///
    /// The panic precedes the `... FAILED` line, which is the whole reason
    /// this fixture exists: the first version of it had them the other way
    /// around, matching how they read rather than how they are written, and
    /// the parser it was testing had the same mistake.
    const OUTPUT: &str = "\
running 3 tests
skipping: no card node on this machine
thread 'the_broken_one' panicked at crates/x/tests/y.rs:12:5:
all 2 acquisitions returned the same image,
so nothing is double buffered
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test a_passing_one ... ok
test the_broken_one ... FAILED

failures:
    the_broken_one

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 1 filtered out;
";

    #[test]
    fn a_failure_carries_the_panic_that_produced_it() {
        let outcome = parse(OUTPUT, true);
        assert_eq!(outcome.passed, 1);
        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.failures.len(), 1, "one failure, named once");
        let failure = &outcome.failures[0];
        assert_eq!(failure.name, "the_broken_one");
        assert_eq!(
            failure.detail,
            vec![
                "at crates/x/tests/y.rs:12:5:".to_string(),
                "all 2 acquisitions returned the same image,".to_string(),
                "so nothing is double buffered".to_string(),
            ],
            "the location and the whole message, stopping before the note"
        );
    }

    #[test]
    fn the_same_failure_named_by_several_targets_is_counted_once() {
        // `--no-fail-fast` prints a trailing `failures:` list per test binary,
        // so a name can appear repeatedly for one failure.
        let doubled = format!("{OUTPUT}\nfailures:\n    the_broken_one\n");
        let outcome = parse(&doubled, true);
        assert_eq!(outcome.failures.len(), 1);
    }

    #[test]
    fn a_skip_is_counted_by_its_reason_without_the_parenthetical() {
        let text = "skipping: no device (llvmpipe)\nskipping: no device (radeonsi)\n";
        let outcome = parse(text, false);
        assert_eq!(outcome.skips.len(), 1, "one reason, two machines");
        assert_eq!(outcome.skips[0].count, 2);
    }
}
