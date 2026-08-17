//! Development task runner, invoked as `cargo xtask <command>`.
//!
//! Implemented:
//!
//! - `report` -- what each device on this machine reports it can do, in the
//!   terms the layers above the HAL actually branch on. `--json` for a machine.
//!
//! Planned:
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

mod report;

const USAGE: &str = "\
cargo xtask <command>

Commands:
  report            What this machine's devices report they can do.
                    --json  emit the same report for a machine to read.
  help              This text.
";

fn main() {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    let rest: Vec<String> = args.collect();

    match command.as_deref() {
        Some("report") => {
            let devices = report::gather();
            if rest.iter().any(|a| a == "--json") {
                print!("{}", report::json(&devices));
            } else {
                print!("{}", report::text(&devices));
            }
        }
        Some("help") | Some("--help") | Some("-h") | None => print!("{USAGE}"),
        Some(other) => {
            // Named rather than merely refused, and alongside what does exist,
            // because the usual reason to be here is a command from the list in
            // this file's header that has not been written yet.
            eprintln!("xtask: no such command: {other}\n\n{USAGE}");
            std::process::exit(1);
        }
    }
}
