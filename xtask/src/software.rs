//! Selecting the CPU implementations of both APIs, the way CI does.
//!
//! This machine's devices do not cover everything the suite can test. Neither
//! the discrete part through Vulkan nor the same part through GLES offers
//! advanced blending, so every scene needing it is skipped here and runs only
//! on the hosted runner, which has no GPU and uses Mesa's CPU drivers instead.
//! That is not a reduced mode -- lavapipe and llvmpipe are conformant, and the
//! runner covers strictly more than the workstation does.
//!
//! Which means a failure that only the runner can see is a failure this machine
//! cannot reproduce, and one of those has already happened: a check on whether
//! a scene drew anything was wrong for a plate that needs advanced blending,
//! and stayed wrong locally through twenty-seven commits because the plate is
//! skipped here.
//!
//! Reproducing it took setting two variables, and setting only the Vulkan one
//! -- which is the obvious half -- gives a *different* wrong answer rather than
//! a partial one: the cross-backend comparison then holds a software rasterizer
//! against a hardware one and reports every antialiased edge as a difference.
//! So this pairs them, or does neither.

use std::path::PathBuf;

/// Where Mesa's Vulkan ICD manifests live.
const ICD_DIR: &str = "/usr/share/vulkan/icd.d";

/// The lavapipe ICD manifest, found rather than assumed.
///
/// The filename carries an architecture suffix that differs between
/// distributions, which is exactly the kind of thing to discover: CI learned
/// this by hardcoding Fedora's name and failing on Ubuntu's.
pub fn lavapipe_icd() -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(ICD_DIR)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            is_lavapipe_manifest(name)
        })
        .collect();
    // Sorted, so a directory holding more than one manifest picks the same one
    // every run rather than whatever the filesystem happened to return first.
    found.sort();
    found.into_iter().next()
}

/// Whether a filename in the ICD directory is lavapipe's manifest.
///
/// Matched on the driver's short name rather than on a whole filename, because
/// the architecture suffix differs between distributions and that is the part
/// CI already got wrong once by hardcoding Fedora's. What must not match is
/// every other driver in that directory -- there are a dozen on this machine,
/// and picking radeon's would run the suite against hardware while announcing
/// that it had not.
fn is_lavapipe_manifest(name: &str) -> bool {
    name.contains("lvp") && name.ends_with(".json")
}

/// The variables that put both backends on the CPU, or why they could not be
/// set.
///
/// Returned as a pair rather than applied here, so the caller can print what it
/// is about to do. A run that silently used a different device than the one
/// named would be the failure this whole module exists to avoid.
pub fn environment() -> Result<Vec<(&'static str, String)>, String> {
    let Some(icd) = lavapipe_icd() else {
        let listing = std::fs::read_dir(ICD_DIR)
            .map(|entries| {
                let mut names: Vec<String> = entries
                    .flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect();
                names.sort();
                names.join(", ")
            })
            .unwrap_or_else(|e| format!("({e})"));
        return Err(format!(
            "no lavapipe ICD in {ICD_DIR}. What is there: {listing}\n\
             Install Mesa's CPU Vulkan driver -- vulkan-lavapipe on Fedora, \
             mesa-vulkan-drivers on Debian and Ubuntu."
        ));
    };
    Ok(vec![
        // Pinned to the one manifest found, rather than letting the loader
        // choose among what is installed: a run that quietly picked the
        // hardware driver would go green having tested the thing this command
        // exists to avoid testing.
        ("VK_DRIVER_FILES", icd.display().to_string()),
        ("LIBGL_ALWAYS_SOFTWARE", "1".to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifests_two_distributions_ship_are_both_recognised() {
        assert!(is_lavapipe_manifest("lvp_icd.x86_64.json"));
        assert!(is_lavapipe_manifest("lvp_icd.aarch64.json"));
        assert!(is_lavapipe_manifest("lvp_icd.i686.json"));
    }

    #[test]
    fn no_other_driver_in_that_directory_is_mistaken_for_it() {
        // The real listing from this workstation. Matching any of these would
        // run the suite against something other than the software reference
        // while printing that it had selected the software reference, which is
        // worse than not offering the command.
        for name in [
            "radeon_icd.x86_64.json",
            "intel_icd.x86_64.json",
            "intel_hasvk_icd.x86_64.json",
            "nouveau_icd.x86_64.json",
            "virtio_icd.x86_64.json",
            "dzn_icd.x86_64.json",
            "asahi_icd.x86_64.json",
            "broadcom_icd.x86_64.json",
            "freedreno_icd.x86_64.json",
            "panfrost_icd.x86_64.json",
            "powervr_mesa_icd.x86_64.json",
        ] {
            assert!(!is_lavapipe_manifest(name), "{name} was taken for lavapipe");
        }
    }

    #[test]
    fn a_manifest_that_is_not_json_is_not_one() {
        // The directory holds whatever a package put there. A backup or a
        // disabled manifest keeps the driver's name and loses its extension,
        // and handing the loader one of those names it a file it cannot parse.
        assert!(!is_lavapipe_manifest("lvp_icd.x86_64.json.rpmsave"));
        assert!(!is_lavapipe_manifest("lvp_icd.x86_64.json.disabled"));
    }
}
