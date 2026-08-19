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

fn parity() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .join("docs/parity.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
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
        _ => panic!("the table is not expected to grow past ninety-nine rows"),
    }
}

#[test]
fn the_parity_tables_prose_summary_counts_its_own_rows_correctly() {
    let doc = parity();
    let rows = rows(&doc);
    let count = |status: &str| rows.iter().filter(|(_, s, _)| s == status).count();

    let claim = format!(
        "Of {} rows across `Canvas` and `Paint`: {} exist, {} are partial, {} are \
         expressible by a caller who assembles them, {} are absent, and {} is out of scope.",
        spell(rows.len()),
        spell(count("yes")),
        spell(count("partial")),
        spell(count("via")),
        spell(count("no")),
        spell(count("out of scope")),
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
