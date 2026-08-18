//! The pure-Rust build requirement, enforced.
//!
//! `deny.toml` describes this ban and calls it automated enforcement. Nothing
//! ran it: `cargo-deny` is not invoked anywhere in the repository, and the
//! version installed on this machine cannot parse an edition-2024 workspace at
//! all. So the rule that governs whether this project cross-compiles to a board
//! without a C toolchain was a comment.
//!
//! This is the same ban expressed as a test, which needs nothing installed and
//! runs with everything else. It does not replace `cargo-deny`, which also
//! checks licenses, advisories and duplicate versions; it covers the one rule
//! that is architectural rather than hygienic.
//!
//! A crate appearing here is a decision, not an accident. Taking a dependency
//! that builds C is how cross-compilation to aarch64 and riscv64 stops being
//! trivial, and the point of failing at merge is that the decision gets made
//! deliberately rather than discovered on a board.

/// Crates whose presence means something in the graph builds native code.
///
/// Named by what they do rather than listed as a blocklist of vendors: each of
/// these exists to compile, locate, or generate bindings to something that is
/// not Rust, so any of them in the graph contradicts the requirement.
const BANNED: &[(&str, &str)] = &[
    ("pkg-config", "locates system libraries at build time"),
    ("cc", "compiles C or C++ during the build"),
    ("cmake", "runs CMake during the build"),
    ("bindgen", "generates bindings from system headers"),
    (
        "system-deps",
        "resolves system libraries through pkg-config",
    ),
    ("vcpkg", "locates system libraries through vcpkg"),
];

/// Every package in the workspace's dependency graph, by name.
///
/// Read from `cargo tree` rather than `cargo metadata`, because the former is
/// line-oriented text and the latter is JSON this crate has no parser for —
/// and taking a serialization dependency to check the dependency policy would
/// be its own small joke.
fn packages() -> Option<Vec<String>> {
    let output = std::process::Command::new(env!("CARGO"))
        .args([
            "tree",
            "--workspace",
            // Build dependencies are the point: a crate that only builds C at
            // build time is exactly what this is looking for, and omitting them
            // would miss every one.
            "--edges",
            "normal,build",
            "--all-features",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !output.status.success() {
        eprintln!(
            "cargo tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_string)
            .collect(),
    )
}

#[test]
fn nothing_in_the_graph_builds_native_code() {
    let Some(packages) = packages() else {
        panic!("could not read the dependency graph, so the ban went unchecked");
    };
    assert!(
        packages.len() > 10,
        "only {} packages came back; the graph was not read properly",
        packages.len()
    );

    let found: Vec<String> = BANNED
        .iter()
        .filter(|(name, _)| packages.iter().any(|p| p == name))
        .map(|(name, why)| format!("{name} ({why})"))
        .collect();
    assert!(
        found.is_empty(),
        "the dependency graph contains crates that build native code: {}\n\
         This is the requirement that keeps cross-compilation to aarch64 and \n\
         riscv64 boards trivial. If the dependency is worth it, that is a \n\
         decision to record in the architecture document rather than a list to \n\
         edit.",
        found.join(", ")
    );
}

#[test]
fn the_ban_would_notice_a_crate_that_is_there() {
    // The check above passes both when the graph is clean and when the reading
    // of it silently returned nothing useful. This names a crate that is
    // certainly present and asserts the same machinery finds it.
    let Some(packages) = packages() else {
        panic!("could not read the dependency graph");
    };
    for expected in ["ash", "glow", "drm"] {
        assert!(
            packages.iter().any(|p| p == expected),
            "{expected} is a dependency of this workspace and the graph reading \
             did not find it, so a banned crate would not be found either"
        );
    }
}
