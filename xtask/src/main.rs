//! Development task runner, invoked as `cargo xtask <command>`.
//!
//! Implemented:
//!
//! - `report` -- what each device on this machine reports it can do, in the
//!   terms the layers above the HAL actually branch on. `--json` for a machine.
//! - `drm` -- whether the direct-scanout lane can run here, and what it would
//!   need. Everything it reads is readable without privilege.
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
//!   board attached. `drm` reports whether that is what this machine needs.
//! - `ci repro <job-id>` -- reproduce a merge-blocking CI failure locally.
//!
//! Session hygiene is a hard requirement for DRM cells: refuse to start if
//! master cannot be acquired, rather than half-running, and restore the
//! previous VT and session state on exit including on panic. An engineer's
//! desktop must survive a failed test run.

mod drm;
mod report;

const USAGE: &str = "\
cargo xtask <command>

Commands:
  report            What this machine's devices report they can do.
                    --json  emit the same report for a machine to read.
  drm               Whether this machine can run the direct-scanout lane.
  help              This text.
";

/// The running kernel's release, which names its module directory.
///
/// Read from the kernel rather than by running `uname`, so this needs nothing
/// on the path. An unreadable one leaves the module directory pointing
/// nowhere, which reports vkms as absent — the same answer a kernel without it
/// gives, and the same advice either way.
fn kernel_release() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

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
        Some("drm") => {
            let survey = drm::survey(
                std::path::Path::new("/dev/dri"),
                std::path::Path::new("/sys/class/drm"),
                std::path::Path::new("/proc/modules"),
                &std::path::Path::new("/lib/modules").join(kernel_release()),
            );
            print!("{}", drm::text(&survey));
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
