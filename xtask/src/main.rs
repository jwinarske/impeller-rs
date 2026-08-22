//! Development task runner, invoked as `cargo xtask <command>`.
//!
//! Implemented:
//!
//! - `report` -- what each device on this machine reports it can do, in the
//!   terms the layers above the HAL actually branch on. `--json` for a machine.
//! - `drm` -- whether the direct-scanout lane can run here, and what it would
//!   need. Everything it reads is readable without privilege.
//! - `verify` -- run the suite and report the skips, which a plain `cargo test`
//!   discards along with the rest of a passing test's output.
//! - `gallery` -- render every corpus scene onto one sheet, because no
//!   comparison in the suite can see a scene both implementations get wrong the
//!   same way.
//! - `gate` -- everything that has to pass before a commit, as one exit code.
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

mod drivers;
mod drm;
mod gallery;
mod gate;
mod report;
mod verify;

const USAGE: &str = "\
cargo xtask <command>

Commands:
  report            What this machine's devices report they can do.
                    --json  emit the same report for a machine to read.
  drm               Whether this machine can run the direct-scanout lane.
  verify            Run the suite and report what did not run. Extra arguments
                    are passed to cargo test.
                    --software  run against Mesa's CPU drivers instead of this
                    machine's, which is what CI uses and covers scenes the
                    hardware here reports as unavailable.
                    Without it the drivers are whatever the environment says.
                    On a machine with many drivers installed, setting
                    VK_DRIVER_FILES is worth doing -- see the loader note in
                    docs/architecture.md.
  gallery [path]    Render every corpus scene onto one sheet to look at.
                    Defaults to corpus.ppm.
  gate              Lint, format, build, the feature matrix and the suite,
                    stopping at the first failure. Exits non-zero if any step
                    did not pass.
                    --software  as for verify, above.
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
        Some("gallery") => {
            let path = rest
                .iter()
                .find(|a| !a.starts_with('-'))
                .cloned()
                .unwrap_or_else(|| "corpus.ppm".to_string());
            match gallery::render(6) {
                Ok(sheet) => {
                    if let Err(e) = gallery::write_ppm(&path, &sheet) {
                        eprintln!("writing {path}: {e}");
                        std::process::exit(1);
                    }
                    println!(
                        "{} scene(s) drawn onto {path} ({}x{})",
                        sheet.drawn, sheet.width, sheet.height
                    );
                    print!("{}", gallery::map(&sheet));
                    if !sheet.borrowed.is_empty() {
                        println!(
                            "{} drawn by a fallback device, not this one: {}",
                            sheet.borrowed.len(),
                            sheet.borrowed.join(", ")
                        );
                    }
                    if !sheet.skipped.is_empty() {
                        println!(
                            "{} not rendered by this device: {}",
                            sheet.skipped.len(),
                            sheet.skipped.join(", ")
                        );
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Some("gate") => {
            if !gate::run(rest.iter().any(|a| a == "--software")) {
                std::process::exit(1);
            }
        }
        Some("verify") => {
            let software = rest.iter().any(|a| a == "--software");
            let passed: Vec<String> = rest.into_iter().filter(|a| a != "--software").collect();
            let env = if software {
                match drivers::environment() {
                    Ok(env) => {
                        for (key, value) in &env {
                            eprintln!("{key}={value}");
                        }
                        env
                    }
                    Err(why) => {
                        eprintln!("{why}");
                        std::process::exit(1);
                    }
                }
            } else {
                Vec::new()
            };
            let outcome = verify::run_with_env(&passed, &env);
            print!("{}", verify::text(&outcome));
            if outcome.broke || outcome.failed > 0 {
                std::process::exit(1);
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
