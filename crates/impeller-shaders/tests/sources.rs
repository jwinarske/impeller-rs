//! What the WGSL sources say about themselves, before naga sees them.
//!
//! Distinct from the snapshots beside this, which pin what naga *produced*.
//! This asks a question about the input, and it exists because the one thing
//! that would have answered it does not: naga emits an uncalled function into
//! the GLSL rather than dropping it, so a helper nothing calls is code every
//! GLES driver compiles for the life of the program.

// Reached from a helper rather than from a test body, so clippy's test-code
// exemption does not see it.
#![allow(clippy::panic)]

use std::path::PathBuf;

fn sources() -> Vec<(String, String)> {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&directory).expect("shaders directory") {
        let path = entry.expect("directory entry").path();
        if path.extension().is_some_and(|e| e == "wgsl") {
            let name = path
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned();
            out.push((name, std::fs::read_to_string(&path).expect("read shader")));
        }
    }
    assert!(!out.is_empty(), "no WGSL sources found in {directory:?}");
    out.sort();
    out
}

/// Comments name functions, so they have to go before anything is counted.
///
/// This file's own convention makes that necessary rather than tidy: a
/// function's doc comment usually names the function under it, and half the
/// prose in `solid.wgsl` names one function while explaining another.
fn without_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every function is called, or is a stage the pipeline enters at.
///
/// A shader is not a program with a linker that drops what nothing reaches.
/// Measured here: `outline_if_asked` in `solid.wgsl` was written with no
/// caller and stayed that way from the commit that added it, and naga put it
/// in the GLSL every time -- two hundred and seven bytes of source and
/// seventy-eight words of SPIR-V that every driver parsed and no fragment ever
/// ran. `docs/on-a-board.md` records that this shader's cost is a step
/// function of its size, which is what makes that worth a test rather than a
/// tidy-up.
///
/// A chain of dead functions is caught one link at a time: a helper called
/// only from an uncalled function still reads as called here. That is the
/// direction to be wrong in -- removing the root makes the next run name the
/// next one -- and it is the whole of what this does not catch.
#[test]
fn every_function_in_a_shader_is_either_called_or_a_stage() {
    let mut dead = Vec::new();
    for (name, source) in sources() {
        let body = without_comments(&source);
        for (index, line) in body.lines().enumerate() {
            let Some(rest) = line.strip_prefix("fn ") else {
                continue;
            };
            let Some(function) = rest.split('(').next() else {
                continue;
            };
            // A stage is entered by the pipeline rather than called, and is
            // marked as one on the line above.
            let entry = index
                .checked_sub(1)
                .and_then(|previous| body.lines().nth(previous))
                .is_some_and(|previous| {
                    previous.contains("@vertex") || previous.contains("@fragment")
                });
            if entry {
                continue;
            }
            let calls = body.matches(&format!("{function}(")).count() - 1;
            if calls == 0 {
                dead.push(format!("{name}: {function}"));
            }
        }
    }
    assert!(
        dead.is_empty(),
        "these shader functions are never called, and naga emits them anyway:\n  {}",
        dead.join("\n  ")
    );
}
