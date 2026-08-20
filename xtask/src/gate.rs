//! Everything that has to pass before a commit, as one exit code.
//!
//! The steps were a list in a document and a sequence typed by hand, which is
//! two ways to half-run them. The failure that prompted this was smaller and
//! more embarrassing than a step being skipped: the sequence printed a marker
//! saying the lint was clean *after* running it, unconditionally, so a real
//! error scrolled past under a line claiming the opposite. A person reading
//! their own output believes the summary.
//!
//! So the steps live here, each one's exit status is the gate, and the first
//! failure stops the rest. There is nothing to remember and nothing to
//! misread.
//!
//! This is not what CI runs. CI runs the same work as separate jobs, which is
//! right there -- they parallelize, and a failure names itself without a log
//! to read. This is for the machine the change is being written on, where the
//! useful property is the opposite: one command, one answer, stopping at the
//! first thing that is wrong.

use std::process::{Command, Stdio};

/// One step of the gate.
struct Step {
    /// What it is for, in the terms the failure will be read in.
    what: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    /// Extra environment, for the steps that need the lint to be fatal.
    env: &'static [(&'static str, &'static str)],
}

const STEPS: &[Step] = &[
    Step {
        // First, because a lint that rewrites code changes what the rest of
        // the gate is checking. `--fix` rather than a bare check: the point is
        // to leave the tree in the state a commit wants, not to describe how
        // far it is from one.
        what: "lint, applying what it can fix",
        program: "cargo",
        args: &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--fix",
            "--allow-dirty",
            "--allow-staged",
        ],
        env: &[("RUSTFLAGS", "-D warnings")],
    },
    Step {
        what: "format",
        program: "cargo",
        args: &["fmt", "--all"],
        env: &[],
    },
    Step {
        // After formatting, because a formatter that changed a file changes
        // what compiles -- and because this is the step whose failure is worth
        // seeing on a tree that is otherwise ready.
        what: "lint again, with warnings fatal",
        program: "cargo",
        args: &["clippy", "--workspace", "--all-targets"],
        env: &[("RUSTFLAGS", "-D warnings")],
    },
    Step {
        what: "build every target",
        program: "cargo",
        args: &["build", "--workspace", "--all-targets"],
        env: &[],
    },
];

/// Feature combinations that must each compile on their own.
///
/// The two axes are meant to compose independently, and the way that stops
/// being true is a crate quietly depending on a backend it did not name. Each
/// of these is a build that would fail if one did.
const FEATURES: &[&str] = &["vulkan", "gles", "vulkan,gles,drm"];

/// Run the gate. Returns whether everything passed.
pub fn run(software: bool) -> bool {
    // Resolved before anything is built, so a missing driver is reported in a
    // second rather than after a full compile.
    let env = if software {
        match crate::software::environment() {
            Ok(env) => {
                println!("== on the CPU implementations of both APIs ==");
                for (key, value) in &env {
                    println!("   {key}={value}");
                }
                env
            }
            Err(why) => {
                eprintln!("{why}");
                return false;
            }
        }
    } else {
        Vec::new()
    };

    for step in STEPS {
        if !step_passed(step.what, step.program, step.args, step.env) {
            return false;
        }
    }

    for features in FEATURES {
        let args = [
            "check",
            "-q",
            "-p",
            "impeller",
            "--no-default-features",
            "--features",
            features,
        ];
        if !step_passed(&format!("build with {features} alone"), "cargo", &args, &[]) {
            return false;
        }
    }

    // Last, and reported by the suite's own summary rather than by an exit
    // code alone: a run that passes but skipped half of itself is the thing
    // the census exists to make visible, and it is worth reading even when the
    // gate is about to say yes.
    println!("== suite, with the skips named ==");
    let outcome = crate::verify::run_with_env(&[], &env);
    print!("{}", crate::verify::text(&outcome));
    !(outcome.broke || outcome.failed > 0)
}

fn step_passed(what: &str, program: &str, args: &[&str], env: &[(&str, &str)]) -> bool {
    println!("== {what} ==");
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for (key, value) in env {
        command.env(key, value);
    }
    match command.status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("\n{what}: failed ({status})");
            false
        }
        Err(e) => {
            eprintln!("\n{what}: could not run {program}: {e}");
            false
        }
    }
}
