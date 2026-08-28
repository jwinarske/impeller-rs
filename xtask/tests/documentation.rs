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
    // A category with nothing in it is left out of the sentence rather than
    // counted at zero. Prose does not say "zero are partial"; it stops
    // mentioning the thing that is not there. Demanding otherwise would put
    // this test in the position of requiring a sentence nobody would write,
    // which is how a document ends up serving its checker instead of a reader.
    let mut clauses = Vec::new();
    if count("yes") > 0 {
        clauses.push(agreeing(count("yes"), "exist"));
    }
    for (n, noun) in [
        (count("partial"), "partial"),
        (count("via"), "expressible by a caller who assembles them"),
        (count("no"), "absent"),
        (count("out of scope"), "out of scope"),
    ] {
        if n > 0 {
            clauses.push(format!("{} {noun}", agreeing(n, "be")));
        }
    }
    let listed = match clauses.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{}, and {last}", rest.join(", ")),
        _ => clauses.join(""),
    };
    let claim = format!(
        "Of {} rows across `Canvas` and `Paint`: {listed}.",
        spell(rows.len()),
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
    // The C++ file each topic mirrors. Primitive shapes are absent because
    // the catalog holds nothing under that topic yet, so there would be
    // nothing to check; the total test still covers what it would contribute
    // once there is.
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
        ("aiks_dl_text_unittests.cc", "text"),
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

/// A skip has to say "skipping", because that is the word the census counts.
///
/// `cargo xtask verify` exists because a skipped test passes and its reason is
/// discarded, and it finds the reasons by looking for that word. A test that
/// announces itself some other way is therefore counted as having run -- which
/// is the exact condition `verify` was written to end, arrived at from the
/// other side.
///
/// Four sites said "no backend available" and one called itself a note. All
/// five were invisible to the census, and one of them had been since it was
/// written.
///
/// The vocabulary below is small on purpose. It is not trying to recognize
/// every sentence a skip might be written in; it recognizes the ones this
/// workspace has actually used, so a sixth site copied from any existing one
/// is caught.
#[test]
fn a_skip_says_the_word_the_census_counts() {
    const SKIP_LIKE: [&str; 6] = [
        "no backend available",
        "unavailable",
        "not installed",
        "not supported",
        "not built",
        "no device",
    ];
    // Its own walk, because `sources` hands back contents and not paths -- and
    // the first draft of this took them for paths, found no file to open, and
    // passed having read nothing. A check that cannot fail is worse than none,
    // so this one counts what it looked at and says so if that is zero.
    fn test_files(dir: &std::path::Path, into: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                test_files(&path, into);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path.display().to_string();
                if name.contains("/tests/") {
                    into.push((name, std::fs::read_to_string(&path).unwrap_or_default()));
                }
            }
        }
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .join("crates");
    let mut files = Vec::new();
    test_files(&root, &mut files);
    assert!(
        files.len() > 10,
        "found {} test files to scan, which cannot be right",
        files.len()
    );

    let mut bare = Vec::new();
    for (path, text) in files {
        for (number, line) in text.lines().enumerate() {
            let Some(rest) = line.split_once("eprintln!(\"") else {
                continue;
            };
            let Some((message, _)) = rest.1.split_once('"') else {
                continue;
            };
            let lower = message.to_lowercase();
            if lower.contains("skipping") {
                continue;
            }
            if SKIP_LIKE.iter().any(|word| lower.contains(word)) {
                bare.push(format!("{path}:{}: {message}", number + 1));
            }
        }
    }
    assert!(
        bare.is_empty(),
        "these read as skips and do not say \"skipping\", so `cargo xtask \
         verify` counts the tests as having run:\n  {}",
        bare.join("\n  ")
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
    const BRITISH: [&str; 9] = [
        "colour",
        "neighbour",
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

/// The scanout seam has to stay free of the library behind it.
///
/// `ScanoutOutput` exists so the KMS library can be replaced without this
/// renderer reimplementing KMS logic, and the architecture says as much: the
/// surface is stated as a trait "so the two projects can be sequenced against
/// each other rather than discovering a mismatch at integration". That is only
/// true while the trait, the types in its signatures, and the frame loop that
/// drives it name no type from whichever library currently implements it.
///
/// It is true today and was measured rather than assumed: every mention of
/// `drm::` in the crate is in the two files implementing the trait. This test
/// is here because that is an invariant a single convenient import would end,
/// silently, in a crate where importing it is otherwise ordinary -- and a
/// second implementation is planned, which is exactly when it would be found
/// out the expensive way.
#[test]
fn the_scanout_trait_names_nothing_from_the_library_behind_it() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("crates/impeller-present-drm/src");

    // Where the binding may be named: the implementation of the trait, and the
    // device handling underneath it. Everything else is the seam.
    let implementation = ["kms.rs", "device.rs"];

    let entries = std::fs::read_dir(&crate_dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", crate_dir.display()));
    let mut checked = 0;
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("file name")
            .to_string();
        if implementation.contains(&name.as_str()) {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        // Prose may discuss the library; code may not name it. The comment
        // marker is enough to tell them apart here, every mention in this crate
        // being either an import or a path in an expression.
        for (number, line) in source.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("*") {
                continue;
            }
            assert!(
                !line.contains("drm::"),
                "{name}:{} names the KMS binding, which the seam may not: {}\n\
                 A type from it in the trait, in a type the trait mentions, or \
                 in the frame loop is what would stop a second implementation \
                 from being a drop-in.",
                number + 1,
                line.trim()
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 3,
        "expected the trait, the frame loop and the crate root at least, \
         checked {checked} files"
    );
}

/// The one program outside the workspace has to reach everything it needs
/// through the facade.
///
/// It is the only consumer this repository has, so it is the only evidence that
/// what a caller can reach is enough to write something real with. That
/// evidence is worth nothing if it is allowed to reach past the facade to the
/// crates underneath, which it did for as long as `present-wsi` was a feature
/// that enabled nothing: it named six of them, and the one it could not do
/// without was the swapchain the facade had no route to.
///
/// So the rule is that it names the facade and nothing else from here, with one
/// exception. `impeller-testkit` is the scene corpus this tool exists to look
/// at, not part of the renderer's surface, and a consumer who is not a test
/// harness would never want it.
#[test]
fn the_playground_reaches_everything_through_the_facade() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("playground/Cargo.toml");
    let source = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("reading {}: {e}", manifest.display()));

    let allowed = ["impeller", "impeller-testkit"];
    let mut named = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        let Some((name, _)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.starts_with("impeller") {
            named.push(name.to_string());
        }
    }
    assert!(
        !named.is_empty(),
        "the playground names no crate from here, which cannot be right"
    );
    for name in &named {
        assert!(
            allowed.contains(&name.as_str()),
            "the playground depends on {name}, reaching past the facade.\n\
             Whatever it needed from there is missing from `impeller`, and \
             adding the dependency hides that instead of fixing it."
        );
    }
}

/// Every divergence recorded says what it costs.
///
/// `docs/non-parity.md` exists so that a difference from upstream is a decision
/// somebody made and can find. A section that says what differs and why, and
/// stops there, leaves the reader to work out whether it matters -- which is
/// the part they came for and the part hardest to reconstruct later. So each
/// numbered entry has to state an impact, including the ones whose impact is
/// nothing.
#[test]
fn every_non_parity_entry_states_its_impact() {
    let text = doc("non-parity.md");
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut heading = String::new();
    let mut body = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if !heading.is_empty() {
                sections.push((heading.clone(), std::mem::take(&mut body)));
            }
            heading = rest.to_string();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    if !heading.is_empty() {
        sections.push((heading, body));
    }

    let numbered: Vec<_> = sections
        .iter()
        .filter(|(heading, _)| heading.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .collect();
    assert!(
        numbered.len() >= 6,
        "docs/non-parity.md has {} numbered entries, which is too few to be the \
         list it claims to be -- did the headings change shape?",
        numbered.len()
    );
    let silent: Vec<&str> = numbered
        .iter()
        .filter(|(_, body)| !body.contains("Impact"))
        .map(|(heading, _)| heading.as_str())
        .collect();
    assert!(
        silent.is_empty(),
        "docs/non-parity.md records a difference without saying what it costs:\n  {}",
        silent.join("\n  ")
    );
}

/// The changelog says of itself that `docs/parity.md` describes what is built
/// "far better than a list could -- both are checked by tests, so neither can
/// drift from the code without failing the build." Four lines below that it
/// kept its own list of two operations it called absent, and went on calling
/// them absent after they were written, because nothing read it.
///
/// So this reads it. Not the whole duplication -- a changelog is allowed to say
/// what an operation's signature used to be, and one paragraph does -- but the
/// one shape that can only ever be wrong: an operation the parity table has
/// verified as built, described here as missing.
#[test]
fn the_changelog_does_not_call_a_built_operation_absent() {
    const ABSENCE: [&str; 5] = ["absent", "absence", "not built", "missing", "unimplemented"];

    let path = repo_root().join("CHANGELOG.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));

    // A cell can name more than one operation, so the names are taken one at a
    // time and with their backticks: `transform` is both a parity row and an
    // ordinary English word, and only the quoted form means the operation.
    let built: Vec<String> = rows(&parity())
        .into_iter()
        .filter(|(_, status, _)| status == "yes")
        .flat_map(|(feature, _, _)| {
            feature
                .split(',')
                .map(|name| name.trim().to_owned())
                .filter(|name| name.starts_with('`') && name.ends_with('`'))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        built.len() >= 40,
        "the parity table yielded {} built rows, which is too few to be the table \
         it was parsed from -- did the columns change shape?",
        built.len()
    );

    let mut stale: Vec<String> = Vec::new();
    for paragraph in text.split("\n\n") {
        let flattened = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
        let lowered = flattened.to_lowercase();
        let Some(word) = ABSENCE.iter().find(|w| lowered.contains(**w)) else {
            continue;
        };
        for feature in &built {
            if flattened.contains(feature.as_str()) {
                stale.push(format!("{feature} -- said to be {word}"));
            }
        }
    }

    assert!(
        stale.is_empty(),
        "CHANGELOG.md describes as unbuilt what docs/parity.md has verified is \
         built:\n  {}\n\
         The parity table owns that claim and a test holds it to the code. A \
         second copy here has nothing holding it to anything, which is how the \
         first one went stale.",
        stale.join("\n  ")
    );
}

/// A run of spaces inside a string literal is alignment or it is a mistake.
///
/// Rust's line continuation swallows the newline *and* the indentation after
/// it, which is why a long message can be written across several lines and
/// still read as one sentence. Drop the backslash and nothing complains: the
/// literal keeps the indentation, and the message reaches a caller with
/// eighteen spaces in the middle of it. One did, in the refusal for asking to
/// draw into an sRGB target, and it read as one sentence in the source the
/// whole time.
///
/// The rule below is what separates the two cases. Padding inside a literal is
/// legitimate when it lines up columns, and something with columns in it has
/// lines: `xtask report` writes rows like `"  dma-buf           import {}"`. A
/// run of spaces in a literal with no newline in it is aligning nothing.
///
/// That is narrower than the mistake, deliberately. A backslash deleted where
/// it stands leaves a real newline in the literal followed by the source's
/// indentation, and this does not catch that: a newline followed by spaces is
/// how several messages here lay out a list, and the shortest indent a dropped
/// backslash leaves is not far above the longest one that is meant. Both
/// instances found so far were the flattened form, which has no such ambiguity
/// -- so this catches the shape it can be sure about rather than guessing at
/// the other, and a reader should not take a pass here as saying no message is
/// wrapped oddly.
#[test]
fn no_message_carries_a_run_of_spaces_where_a_line_continuation_was_dropped() {
    let mut flattened: Vec<String> = Vec::new();
    let mut seen = 0usize;
    for source in sources() {
        for literal in string_literals(&source) {
            seen += 1;
            if !literal.contains('\n') && literal.trim().contains("   ") {
                flattened.push(literal);
            }
        }
    }
    // A scanner that found nothing would pass this and say nothing, which is
    // the shape of check this file exists to distrust. Both instances the rule
    // caught were found by running it, so the floor is set where a scanner that
    // still walks the tree stays above it and one that stopped does not.
    assert!(
        seen > 3_000,
        "the scanner found {seen} string literals across the workspace, which is \
         too few to have read it -- the check would pass whatever the sources say"
    );
    assert!(
        flattened.is_empty(),
        "these string literals carry a run of spaces and have no line to align \
         to, which is what a dropped `\\` leaves behind:\n  {}",
        flattened.join("\n  ")
    );
}

/// Every ordinary string literal in a source file, as Rust will have it.
///
/// Escapes are resolved far enough for the caller above and no further: `\n`
/// becomes a newline so a literal with lines in it can be recognized, a line
/// continuation eats the newline and the indentation after it the way the
/// compiler does, and every other escape yields the character it escaped so
/// that `\"` cannot end a literal early.
///
/// Comments are skipped, both kinds, because a quotation mark inside one would
/// otherwise open a literal that runs to the next unrelated quote and swallow
/// the source between them. Raw strings are skipped rather than returned: the
/// tree's are shader source and SQL-shaped fixtures, none of which is a message
/// anyone reads, and taking them properly means matching hash counts.
fn string_literals(source: &str) -> Vec<String> {
    let bytes: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            '/' if bytes.get(i + 1) == Some(&'/') => {
                while i < bytes.len() && bytes[i] != '\n' {
                    i += 1;
                }
            }
            '/' if bytes.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < bytes.len() && !(bytes[i] == '*' && bytes.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            'r' if matches!(bytes.get(i + 1), Some('"') | Some('#')) => {
                let mut hashes = 0;
                let mut j = i + 1;
                while bytes.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if bytes.get(j) != Some(&'"') {
                    i += 1;
                    continue;
                }
                let close: String = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                let rest: String = bytes[j + 1..].iter().collect();
                i = match rest.find(&close) {
                    Some(at) => j + 1 + rest[..at].chars().count() + close.chars().count(),
                    None => bytes.len(),
                };
            }
            // A lifetime or a character literal, not the start of a string.
            '\'' => {
                i += if bytes.get(i + 1) == Some(&'\\') {
                    4
                } else {
                    3
                }
            }
            '"' => {
                i += 1;
                let mut literal = String::new();
                while i < bytes.len() && bytes[i] != '"' {
                    if bytes[i] != '\\' {
                        literal.push(bytes[i]);
                        i += 1;
                        continue;
                    }
                    i += 1;
                    match bytes.get(i) {
                        Some('n') => literal.push('\n'),
                        // The continuation: the newline goes, and so does the
                        // indentation that follows it. This is the whole point.
                        Some('\n') => {
                            i += 1;
                            while bytes.get(i).is_some_and(|c| c.is_whitespace()) {
                                i += 1;
                            }
                            continue;
                        }
                        Some(other) => literal.push(*other),
                        None => break,
                    }
                    i += 1;
                }
                out.push(literal);
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}
