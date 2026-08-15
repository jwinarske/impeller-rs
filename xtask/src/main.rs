//! Development task runner, invoked as `cargo xtask <command>`.
//!
//! Planned commands:
//!
//! - `device list` / `device run <board> --suite <s> --cell <c>` -- build the
//!   testkit runner for a target, push it to a board, run suites, and pull
//!   back JSON and JUnit reports. The same harness serves an engineer with a
//!   board on the desk and the CI-triggered device rack.
//! - `device baseline <board> --accept` -- re-baseline tolerances as a
//!   reviewed diff.
//! - `device report --last` -- open the report from the most recent run.
//! - `vkms up` -- load VKMS so the full DRM suite runs on a machine with no
//!   board attached.
//! - `ci repro <job-id>` -- reproduce a merge-blocking CI failure locally.
//!
//! Session hygiene is a hard requirement for DRM cells: refuse to start if
//! master cannot be acquired, rather than half-running, and restore the
//! previous VT and session state on exit including on panic. An engineer's
//! desktop must survive a failed test run.

fn main() {
    eprintln!("xtask: no commands implemented yet");
    std::process::exit(1);
}
