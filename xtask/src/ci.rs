//! What the remote said about a commit, for the gate to say out loud.
//!
//! A green gate is not a green CI, and `README.md` sets out why: CI runs the
//! same suite against Mesa's software drivers with the Vulkan validation layer
//! installed, so it sees a device this machine does not have and reports object
//! lifetimes nothing here reports. That is written down, and writing it down
//! was not enough -- CI went twenty-six commits red on one assertion while
//! every local gate passed, because nobody looked.
//!
//! So the gate looks. Not to gate on it: this asks a network for the state of a
//! run that may not have started, and a check that cannot be made offline
//! cannot be a check. It is a line beside the skip census and the timing drift,
//! which are the other two things the gate reports and does not enforce.
//!
//! Best effort throughout. No `gh`, no network, no remote, a run still going --
//! every one of those is a line saying so, never a failure and never silence.
//! Silence is the state this exists to prevent.

use std::process::Command;

/// How the most recent finished run on this branch ended, and at which commit.
struct Run {
    conclusion: String,
    sha: String,
}

fn gh(args: &[&str]) -> Option<String> {
    let out = Command::new("gh").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Which conclusions are an answer about the code.
///
/// A positive list rather than a negative one, and that is the safer direction
/// as well as the shorter: a status nobody here has seen reads as "no answer"
/// and prints the line saying so, where a negative list would report it as
/// whatever it happened to say. A run stopped because a later push superseded
/// it is the common case and tells you nothing about either commit.
const ANSWERS: [&str; 4] = ["success", "failure", "timed_out", "startup_failure"];

/// The last run that reached one of those, ignoring the rest.
///
/// Parsed from a tab-separated list rather than JSON, to keep this from needing
/// a parser for one field of one command.
fn last_conclusive() -> Option<Run> {
    let listed = gh(&[
        "run",
        "list",
        "--limit",
        "20",
        "--json",
        "conclusion,headSha",
        "--jq",
        r#".[] | "\(.conclusion)\t\(.headSha)""#,
    ])?;
    listed.lines().find_map(|line| {
        let (conclusion, sha) = line.split_once('\t')?;
        ANSWERS.contains(&conclusion).then(|| Run {
            conclusion: conclusion.to_string(),
            sha: sha.to_string(),
        })
    })
}

/// The line the gate prints, or one saying why there is none.
pub fn line() -> String {
    let run = last_conclusive();
    let behind = run.as_ref().and_then(|run| {
        git(&["rev-list", "--count", &format!("{}..HEAD", run.sha)])
            .and_then(|n| n.parse::<usize>().ok())
    });
    describe(run.as_ref(), behind)
}

/// The same, from an answer already in hand.
///
/// Split out so the wording can be tested without a network, a remote or a
/// `gh`, which is most of what this file has to get right: every branch here is
/// something the gate will print on somebody's machine.
fn describe(run: Option<&Run>, behind: Option<usize>) -> String {
    let Some(run) = run else {
        return "CI was not asked -- no `gh`, no network, or no run to report. \
                A green gate is not a green CI; see README.md.\n"
            .to_string();
    };
    let short = run.sha.get(..7).unwrap_or(&run.sha);
    let since = match behind {
        Some(0) => "this commit".to_string(),
        Some(1) => format!("{short}, one commit back"),
        Some(n) => format!("{short}, {n} commits back"),
        // A commit the local tree has never seen: a run from another branch, or
        // a history that has been rewritten under it.
        None => format!("{short}, which is not in this history"),
    };
    match run.conclusion.as_str() {
        "success" => format!("CI last passed at {since}\n"),
        other => format!(
            "CI last {other} at {since}. `cargo xtask gate --software` runs what \
             it runs; `gh run view --log-failed` says what it saw.\n"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(conclusion: &str) -> Run {
        Run {
            conclusion: conclusion.to_string(),
            sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
        }
    }

    /// Not being able to ask is a line, not a silence.
    ///
    /// The whole point of this file is that nobody looked for twenty-six
    /// commits, so the one outcome it must not have is printing nothing.
    #[test]
    fn a_missing_answer_says_so() {
        let said = describe(None, None);
        assert!(said.contains("not asked"), "{said}");
        assert!(said.ends_with('\n'), "{said}");
    }

    #[test]
    fn a_pass_at_the_tip_says_this_commit() {
        let said = describe(Some(&run("success")), Some(0));
        assert_eq!(said, "CI last passed at this commit\n");
    }

    /// A number of commits, and the singular said as a word.
    #[test]
    fn a_pass_behind_the_tip_counts_the_commits() {
        assert!(describe(Some(&run("success")), Some(1)).contains("one commit back"));
        assert!(describe(Some(&run("success")), Some(9)).contains("9 commits back"));
    }

    /// Anything but a pass names what to run next, because the gate that
    /// prints this is the place somebody is already sitting.
    #[test]
    fn a_failure_says_what_to_do_about_it() {
        let said = describe(Some(&run("failure")), Some(3));
        assert!(said.contains("CI last failure"), "{said}");
        assert!(said.contains("gate --software"), "{said}");
        assert!(said.contains("--log-failed"), "{said}");
    }

    /// Only the statuses that say something about the code are answers, and
    /// the rest read as none. A run stopped because a later push superseded it
    /// is the common one, and reporting its status would be reporting on a
    /// commit nobody asked about.
    #[test]
    fn only_a_real_conclusion_is_an_answer() {
        for answer in ANSWERS {
            assert!(!describe(Some(&run(answer)), Some(0)).contains("not asked"));
        }
        // `last_conclusive` is what drops the rest; this pins the list it drops
        // them against, which is the part a reader would otherwise have to
        // infer from a `jq` filter.
        assert!(!ANSWERS.contains(&"skipped"));
        assert!(!ANSWERS.contains(&"neutral"));
    }

    /// A run on a commit this tree has never seen says that rather than a
    /// count, which is what a rewritten history or another branch looks like.
    #[test]
    fn a_run_outside_this_history_says_so() {
        let said = describe(Some(&run("success")), None);
        assert!(said.contains("not in this history"), "{said}");
    }
}
