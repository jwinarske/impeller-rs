//! The parity table, checked against the thing it describes.
//!
//! `docs/parity.md` makes two kinds of claim that can be wrong quietly. It
//! cites corpus scenes as evidence that a row exists, and those citations were
//! verified once, by hand, on the day the table was written; a scene renamed
//! afterwards leaves a claim pointing at nothing. And it counts its own rows in
//! prose, which is a summary of a table sitting directly above it and therefore
//! a summary that can disagree with it.
//!
//! Both were checked by hand once. Doing it by hand once is how a document
//! starts out true and stops being true, and this table is the answer given to
//! anyone asking what the renderer can do -- so a stale citation is worse than
//! no citation, since it reads as evidence.
//!
//! What this does not check is whether a cited scene actually exercises the row
//! it is cited under. That is a judgment about what a picture demonstrates, and
//! no test makes it. The mechanical part is still worth having: it is the part
//! that rots on its own, without anybody touching the document.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// The workspace root, which is where every path in this file is relative to.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .to_path_buf()
}

fn doc(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .join("docs")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn parity() -> String {
    doc("parity.md")
}

/// The table rows, as `(feature, status, citations)`.
///
/// A row is a line of four cells. Header and separator lines have that shape
/// too, so they are dropped by their contents rather than by position -- the
/// file holds two tables, and counting lines from the top would only work for
/// the first one.
fn rows(doc: &str) -> Vec<(String, String, Vec<String>)> {
    doc.lines()
        .filter(|line| line.starts_with('|'))
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            if cells.len() != 6 {
                return None;
            }
            let status = cells[2];
            if status == "Status" || status.starts_with("---") {
                return None;
            }
            let cited = cells[4]
                .split(',')
                .filter_map(|c| c.trim().strip_prefix('`')?.strip_suffix('`'))
                .map(str::to_owned)
                .collect();
            Some((cells[1].to_owned(), status.to_owned(), cited))
        })
        .collect()
}

/// Every scene name a citation could refer to.
fn scene_names() -> BTreeSet<String> {
    impeller_testkit::corpus()
        .into_iter()
        .map(|scene| scene.name.to_owned())
        .collect()
}

/// Every Rust source file under `crates`.
fn sources() -> Vec<String> {
    fn walk(dir: &std::path::Path, into: &mut Vec<String>) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, into);
            } else if path.extension().is_some_and(|e| e == "rs") {
                into.push(std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .join("crates");
    let mut files = Vec::new();
    walk(&root, &mut files);
    assert!(
        !files.is_empty(),
        "no sources found under {}",
        root.display()
    );
    files
}

#[test]
fn every_scene_the_parity_table_cites_as_evidence_exists() {
    let scenes = scene_names();
    let sources = sources();
    let mut dangling = Vec::new();

    for (feature, _, cited) in rows(&parity()) {
        for citation in cited {
            // A citation ending in `*` stands for a family -- `gradient-*` is
            // every gradient scene -- and is satisfied by any one of them.
            let found = match citation.strip_suffix('*') {
                Some(prefix) => scenes.iter().any(|name| name.starts_with(prefix)),
                // A row whose evidence is an assertion rather than a picture
                // cites the test by name instead, so a citation is satisfied by
                // either. Matching a definition and not merely the text means a
                // local variable inside an unrelated test does not qualify --
                // which is what three rows here were citing, and how a reader
                // following the citation ended up somewhere that proved
                // something else.
                None => {
                    scenes.contains(&citation)
                        || sources
                            .iter()
                            .any(|src| src.contains(&format!("fn {citation}(")))
                }
            };
            if !found {
                dangling.push(format!("{feature} cites {citation}"));
            }
        }
    }

    assert!(
        dangling.is_empty(),
        "docs/parity.md cites evidence that does not exist:\n  {}\n\
         A citation names a corpus scene or a test function. Either it was \
         renamed and the citation should follow it, or the evidence for that row \
         is gone and the row's status is now a claim with nothing behind it.",
        dangling.join("\n  ")
    );
}

/// English for a number, for numbers a table of features can reach.
fn spell(n: usize) -> String {
    const ONES: [&str; 20] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ];
    const TENS: [&str; 10] = [
        "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];
    match n {
        0..=19 => ONES[n].to_owned(),
        20..=99 if n % 10 == 0 => TENS[n / 10].to_owned(),
        20..=99 => format!("{}-{}", TENS[n / 10], ONES[n % 10]),
        // Hundreds arrived when the catalog passed a hundred plates, which the
        // parity table alone would never have done. The panic below was worded
        // for that table and was reached by the other document -- a reminder
        // that a helper stops being about the thing it was written for the
        // moment a second caller uses it.
        100..=999 if n % 100 == 0 => format!("{} hundred", ONES[n / 100]),
        100..=999 => format!("{} hundred and {}", ONES[n / 100], spell(n % 100)),
        _ => panic!("no document here counts past nine hundred and ninety-nine"),
    }
}

#[test]
fn the_parity_tables_prose_summary_counts_its_own_rows_correctly() {
    let doc = parity();
    let rows = rows(&doc);
    let count = |status: &str| rows.iter().filter(|(_, s, _)| s == status).count();

    // The document is prose and has to read as prose, so a count of one takes a
    // singular verb. Written with `are` throughout, this test would demand a
    // sentence saying "one are partial" -- and the way that gets resolved under
    // time pressure is by writing the ungrammatical sentence, since the test is
    // the thing that has to pass.
    let agreeing = |n: usize, verb: &str| {
        let (singular, plural) = match verb {
            "exist" => ("exists", "exist"),
            _ => ("is", "are"),
        };
        format!("{} {}", spell(n), if n == 1 { singular } else { plural })
    };
    let claim = format!(
        "Of {} rows across `Canvas` and `Paint`: {}, {} partial, {} \
         expressible by a caller who assembles them, {} absent, and {} out of scope.",
        spell(rows.len()),
        agreeing(count("yes"), "exist"),
        agreeing(count("partial"), "be"),
        agreeing(count("via"), "be"),
        agreeing(count("no"), "be"),
        agreeing(count("out of scope"), "be"),
    );

    // The sentence is wrapped in the source, so compare against the document
    // with its line breaks flattened rather than requiring a particular wrap.
    let flattened = doc.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flattened.contains(&claim),
        "docs/parity.md summarizes its own table wrongly. The table says:\n  {claim}\n\
         Adding or changing a row means changing that sentence too -- it is the \
         only part of the document a reader takes on trust."
    );
}

#[test]
fn a_row_claiming_the_operation_exists_names_the_evidence() {
    // The document states this rule about itself: "A row claiming *yes* with no
    // evidence is a bug in this table." Two rows broke it -- both accessors,
    // both genuinely tested, neither citing the test that covered it -- so the
    // rule was true as a policy and false as a description.
    let uncited: Vec<String> = rows(&parity())
        .into_iter()
        .filter(|(_, status, cited)| status == "yes" && cited.is_empty())
        .map(|(feature, _, _)| feature)
        .collect();

    assert!(
        uncited.is_empty(),
        "docs/parity.md claims these exist without naming what renders or asserts \
         them:\n  {}\n\
         An uncited yes is the failure mode the table was written to avoid: it \
         reads as verified and is only asserted.",
        uncited.join("\n  ")
    );
}

#[test]
fn the_architecture_states_the_material_size_the_code_enforces() {
    // The document said a hundred and twelve bytes and the assertion in the
    // code said a hundred and twenty-eight. Both numbers were written
    // deliberately; one of them stopped being true when the layout grew, and
    // nothing anywhere connected the two.
    //
    // This is the number the whole push-constant argument rests on -- the
    // reason the stop count is four, the reason an image material was a
    // question, the reason a conical gradient had to find a spare float. A
    // reader checking that argument against the wrong figure would conclude
    // there was room to spare.
    let bytes = impeller_hal::MATERIAL_FLOATS * 4;
    let stated = format!("packed into {bytes} bytes");
    let flattened = doc("architecture.md")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        flattened.contains(&stated),
        "docs/architecture.md does not say the material is {bytes} bytes, which is \
         what MATERIAL_FLOATS makes it. Changing the layout means changing the \
         sentence that explains why the layout is the size it is."
    );
}

#[test]
fn the_playground_inventory_counts_the_catalog_correctly() {
    // The other numbers in that document describe a source tree this
    // repository does not contain, and it says so. This one describes the
    // collection right here, which makes it the one number a reader would be
    // entitled to trust -- so it is the one that is checked.
    let total = impeller_testkit::catalog().len();
    let flattened = doc("playground-parity.md")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let stated = format!("holds {} scenes of roughly", spell(total));
    assert!(
        flattened.contains(&stated),
        "docs/playground-parity.md does not say the catalog holds {total} scenes. \
         Adding one means saying so, or the inventory stops being an inventory."
    );
}

#[test]
fn the_playground_inventory_counts_each_file_correctly() {
    // The total alone is too forgiving a check: a per-file number drifted by
    // twelve while the total stayed right, because the two were edited
    // separately and only one of them was under test. Every number that
    // describes this repository is checked here, so the table cannot be
    // partially true.
    let by_topic = |topic: &str| {
        impeller_testkit::catalog()
            .iter()
            .filter(|scene| {
                scene
                    .name
                    .split_once('/')
                    .is_some_and(|(prefix, _)| prefix == topic)
            })
            .count()
    };
    // The C++ file each topic mirrors. Files with no counterpart here --
    // text, primitive shapes -- are absent because they have nothing to
    // check; the total test still covers what they would contribute.
    let mirrors = [
        ("aiks_dl_basic_unittests.cc", "basic"),
        ("aiks_dl_path_unittests.cc", "path"),
        ("aiks_dl_gradient_unittests.cc", "gradient"),
        ("aiks_dl_clip_unittests.cc", "clip"),
        ("aiks_dl_opacity_unittests.cc", "opacity"),
        ("aiks_dl_blend_unittests.cc", "blend"),
        ("aiks_dl_blur_unittests.cc", "blur"),
        ("aiks_dl_vertices_unittests.cc", "vertices"),
        ("aiks_dl_atlas_unittests.cc", "atlas"),
        ("aiks_dl_shadow_unittests.cc", "shadow"),
        ("aiks_dl_unittests.cc", "dl"),
        ("aiks_dl_runtime_effect_unittests.cc", "effect"),
    ];
    let doc = doc("playground-parity.md");
    let mut counted = 0;
    for (file, topic) in mirrors {
        let row = doc
            .lines()
            .find(|line| line.starts_with(&format!("| `{file}` |")))
            .unwrap_or_else(|| panic!("docs/playground-parity.md has no row for {file}"));
        let stated: usize = row
            .split('|')
            .nth(3)
            .and_then(|cell| cell.trim().parse().ok())
            .unwrap_or_else(|| panic!("the {file} row does not state a count: {row}"));
        let actual = by_topic(topic);
        assert_eq!(
            stated, actual,
            "docs/playground-parity.md says {stated} scenes mirror {file}, \
             but the catalog holds {actual} under {topic}/"
        );
        counted += actual;
    }
    let total = impeller_testkit::catalog().len();
    assert_eq!(
        counted, total,
        "the catalog holds {total} scenes but only {counted} fall under a topic \
         the inventory names, so some scene is uncounted by the table"
    );
}

#[test]
fn the_tree_is_written_in_american_english() {
    // A stated convention for this project, and one that drifts silently:
    // nothing about "colour" beside "color" fails to compile, and a message a
    // caller reads is as much a part of the interface as the name it belongs
    // to. Eighty-odd of these accumulated before anyone looked, in comments, in
    // test names, and in the text of two errors that sat next to their
    // American twins.
    //
    // Checked by spelling rather than by a dictionary, because the list of
    // words this project actually uses is short and a dictionary would need one
    // anyway. None of these is a substring of an American word, which is what
    // makes an unanchored search the right one -- "recolours" has to be found
    // as surely as "colours".
    const BRITISH: [&str; 8] = [
        "colour",
        "centre",
        "behaviour",
        "recognise",
        "normalise",
        "favour",
        "modelled",
        "cancelled",
    ];
    // Everything tracked that a reader or a caller sees. The two untracked
    // files are excluded by not being here.
    let roots = ["crates", "xtask", "docs", "playground", "tests"];
    let mut found = Vec::new();
    for root in roots {
        let mut stack = vec![repo_root().join(root)];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n == "target") {
                        continue;
                    }
                    stack.push(path);
                    continue;
                }
                let is_text = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e, "rs" | "md" | "wgsl" | "toml" | "yml"));
                if !is_text {
                    continue;
                }
                // This file names every word it forbids, so it answers to all
                // of them. A check that has to spell out what it is looking for
                // cannot also be looked in.
                if path.file_name().is_some_and(|n| n == "documentation.rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let lower = text.to_lowercase();
                for word in BRITISH {
                    if lower.contains(word) {
                        found.push(format!("{}: {word}", path.display()));
                    }
                }
            }
        }
    }
    found.sort();
    found.dedup();
    assert!(
        found.is_empty(),
        "this project is written in American English, and these are not:\n  {}",
        found.join("\n  ")
    );
}
