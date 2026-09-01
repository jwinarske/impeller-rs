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

include!(concat!(env!("OUT_DIR"), "/sources.rs"));

/// Every WGSL source, by file name.
///
/// From the build rather than from the directory, and the reason is a board: a
/// test binary is cross-built and copied to a device with no source tree, where
/// `CARGO_MANIFEST_DIR` names a path on the machine that compiled it. `build.rs`
/// walks the directory once for both jobs, so a shader added to it is covered
/// here without being added anywhere.
fn sources() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = SHADER_SOURCES
        .iter()
        .map(|(name, text)| ((*name).to_string(), (*text).to_string()))
        .collect();
    assert!(!out.is_empty(), "the build emitted no shader sources");
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

/// Every material kind the renderer can ask for is one the shader answers, and
/// the other way round.
///
/// Two lists that have to agree and nothing making them: `Material::kind` in
/// `impeller-hal` names fourteen codes, and `shade` in `solid.wgsl` decides
/// what to do with a number. A code with no arm draws whatever the fall-through
/// leaves, which is a wrong picture rather than an error. An arm with no code
/// is a branch no fragment reaches, which the test above would not catch --
/// dead code inside a live function rather than a function nothing calls.
///
/// The dispatch is read for the numbers it tests against rather than for its
/// shape, because it has two shapes and has had three. A `switch` names its
/// arms as `case 4:`; the gradients ahead of it are an `if` chain comparing
/// against half-integers, because they share a tail the switch cannot; and the
/// solid case is a bare early return. What is common to all of them is that the
/// number appears, so that is what is looked for.
#[test]
fn the_shader_answers_for_every_material_kind_and_no_others() {
    use impeller_hal::material::kind;

    // Written out rather than derived, because there is nothing to derive
    // from: these are `pub const f32` in a module, and a module is not
    // enumerable. The count is asserted below so that a constant added there
    // and not here is caught rather than silently uncovered.
    let named: [(&str, f32); 14] = [
        ("SOLID", kind::SOLID),
        ("LINEAR", kind::LINEAR),
        ("RADIAL", kind::RADIAL),
        ("SWEEP", kind::SWEEP),
        ("IMAGE", kind::IMAGE),
        ("GLYPH", kind::GLYPH),
        ("BLUR", kind::BLUR),
        ("ROUNDED_RECT", kind::ROUNDED_RECT),
        ("ELLIPSE", kind::ELLIPSE),
        ("CONICAL", kind::CONICAL),
        ("MESH", kind::MESH),
        ("MORPHOLOGY", kind::MORPHOLOGY),
        ("ROUNDED_RECT_BLUR", kind::ROUNDED_RECT_BLUR),
        ("POINT_FIELD", kind::POINT_FIELD),
    ];

    let solid = sources()
        .into_iter()
        .find(|(name, _)| name == "solid.wgsl")
        .expect("solid.wgsl");
    let body = without_comments(&solid.1);
    let dispatch = body
        .split_once("fn shade(")
        .expect("solid.wgsl has a `shade`")
        .1;

    // A kind is answered if the dispatch tests against it: as `case 7:` in the
    // switch, or as the half-integers either side of it in the chain before.
    let answered = |code: f32| {
        let n = code as i32;
        dispatch.contains(&format!("case {n}:"))
            || (dispatch.contains(&format!("kind > {}.5", n - 1))
                && dispatch.contains(&format!("kind < {n}.5")))
            || (n == 0 && dispatch.contains("kind < 0.5"))
    };

    let unanswered: Vec<&str> = named
        .iter()
        .filter(|(_, code)| !answered(*code))
        .map(|(name, _)| *name)
        .collect();
    assert!(
        unanswered.is_empty(),
        "the shader has no arm for {unanswered:?}, so a draw asking for one \
         gets whatever the fall-through leaves"
    );

    // And the other way: every `case` in the switch is a code something can
    // ask for. The chain before it is not read for this, since its bounds are
    // a range rather than a value and a spurious one would have to be written
    // deliberately.
    let mut orphans = Vec::new();
    for (index, _) in dispatch.match_indices("case ") {
        let rest = &dispatch[index + "case ".len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let Ok(code) = digits.parse::<i32>() else {
            continue;
        };
        if !named.iter().any(|(_, c)| *c as i32 == code) {
            orphans.push(code);
        }
    }
    assert!(
        orphans.is_empty(),
        "the shader has arms for {orphans:?}, which no material kind names -- \
         a branch every driver compiles and no fragment reaches"
    );

    // And the list above is the whole of `kind` rather than a sample of it,
    // which is the assertion that keeps the two before it honest. Read from the
    // module's own source, because a list checked against itself cannot fail:
    // the first version of this compared the highest code it had been given
    // against the length of the list it had been given, and adding a constant
    // to `impeller-hal` passed it.
    //
    // `include_str!` rather than a read, for the reason `sources` gives.
    const MATERIAL: &str = include_str!("../../impeller-hal/src/material.rs");
    let module = MATERIAL
        .split_once("pub mod kind {")
        .expect("a `kind` module")
        .1
        .split_once("\n}")
        .expect("the end of it")
        .0;
    let declared: Vec<&str> = module
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub const "))
        .filter_map(|rest| rest.split(':').next())
        .collect();
    let missing: Vec<&str> = declared
        .iter()
        .filter(|name| !named.iter().any(|(known, _)| known == *name))
        .copied()
        .collect();
    assert!(
        missing.is_empty(),
        "`impeller_hal::material::kind` declares {missing:?}, which the list in \
         this test does not name -- so nothing here asks whether the shader \
         answers for them"
    );
}
