//! Naming the drivers the suite runs against, rather than letting the loader
//! choose.
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
//!
//! # Why the ordinary run names its drivers too
//!
//! A segmentation fault in the suite was diagnosed to the Vulkan loader
//! unloading driver libraries on one thread while another looked a device up in
//! them. It runs that unload for every ICD that reports no device, and this
//! machine has twelve manifests installed of which two find anything. Naming
//! the two leaves the loader nothing to scan and nothing to unload.
//!
//! Both of them, not one. The conformance suite compares a scene across
//! devices, and the two it uses are the hardware Vulkan driver and the software
//! reference -- so naming only the first would trade a crash for a test that
//! silently stopped comparing anything. `docs/architecture.md` has the core
//! dump this came from.

use std::path::PathBuf;

/// Where Mesa's Vulkan ICD manifests live.
const ICD_DIR: &str = "/usr/share/vulkan/icd.d";

/// The ICD manifest whose driver name contains `driver`, found rather than
/// assumed.
///
/// The filename carries an architecture suffix that differs between
/// distributions, which is exactly the kind of thing to discover: CI learned
/// this by hardcoding Fedora's name and failing on Ubuntu's.
pub fn icd(driver: &str) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(ICD_DIR)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            is_manifest_for(name, driver)
        })
        .collect();
    // Sorted, so a directory holding more than one manifest picks the same one
    // every run rather than whatever the filesystem happened to return first.
    found.sort();
    found.into_iter().next()
}

/// The lavapipe ICD manifest.
pub fn lavapipe_icd() -> Option<PathBuf> {
    icd("lvp")
}

/// Whether a filename in the ICD directory is `driver`'s manifest.
///
/// Matched on the driver's short name rather than on a whole filename, because
/// the architecture suffix differs between distributions and that is the part
/// CI already got wrong once by hardcoding Fedora's. What must not match is
/// every other driver in that directory -- there are a dozen on this machine,
/// and picking radeon's when lavapipe was asked for would run the suite against
/// hardware while announcing that it had not.
fn is_manifest_for(name: &str, driver: &str) -> bool {
    name.contains(driver) && name.ends_with(".json")
}

/// Which drivers a run should name.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The drivers that actually have a device here: the hardware Vulkan one
    /// and the software reference. Both, because the conformance suite compares
    /// a scene across the two of them.
    Devices,
    /// The CPU implementations of both APIs, which is what CI uses.
    Software,
}

/// The hardware Vulkan drivers worth naming, in the order they are preferred.
///
/// A short list rather than a discovery, and the honest reason is that
/// discovering it means enumerating devices, which is the call that crashes.
/// The first present wins; if none is, the loader is left to scan as it did
/// before and the run says so.
const HARDWARE: [&str; 4] = ["radeon", "intel", "nouveau", "virtio"];

/// The variables that put both backends on the CPU, or why they could not be
/// set.
///
/// Returned as a pair rather than applied here, so the caller can print what it
/// is about to do. A run that silently used a different device than the one
/// named would be the failure this whole module exists to avoid.
pub fn environment(mode: Mode) -> Result<Vec<(&'static str, String)>, String> {
    if mode == Mode::Devices {
        // The software reference is named alongside the hardware driver rather
        // than instead of it, because the conformance suite renders the same
        // scene on both and compares. Naming one would leave that test with a
        // single device and nothing to compare, which is a quieter failure than
        // the crash it is avoiding.
        let mut named: Vec<String> = Vec::new();
        if let Some(path) = HARDWARE.iter().find_map(|driver| icd(driver)) {
            named.push(path.display().to_string());
        }
        if let Some(path) = lavapipe_icd() {
            named.push(path.display().to_string());
        }
        if named.is_empty() {
            // Nothing recognized, so nothing is named and the loader scans as
            // it always did. Refusing here would turn a machine this list does
            // not know about into a machine the suite will not run on.
            return Ok(Vec::new());
        }
        return Ok(vec![("VK_DRIVER_FILES", named.join(":"))]);
    }

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
    fn the_manifests_two_distributions_ship_are_both_recognized() {
        for name in [
            "lvp_icd.x86_64.json",
            "lvp_icd.aarch64.json",
            "lvp_icd.i686.json",
        ] {
            assert!(
                is_manifest_for(name, "lvp"),
                "{name} was not taken for lavapipe"
            );
        }
        for name in ["radeon_icd.x86_64.json", "radeon_icd.aarch64.json"] {
            assert!(
                is_manifest_for(name, "radeon"),
                "{name} was not taken for radeon"
            );
        }
    }

    #[test]
    fn no_other_driver_in_that_directory_is_mistaken_for_one() {
        // The real listing from this workstation. Matching any of these when
        // lavapipe was asked for would run the suite against something other
        // than the software reference while printing that it had selected the
        // software reference, which is worse than not offering the command.
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
            assert!(
                !is_manifest_for(name, "lvp"),
                "{name} was taken for lavapipe"
            );
        }
        // And the other way, which is the pairing the ordinary run depends on:
        // naming the hardware driver must not name the software one as well,
        // or the two entries in the list would be the same manifest twice.
        assert!(!is_manifest_for("lvp_icd.x86_64.json", "radeon"));
    }

    #[test]
    fn a_manifest_that_is_not_json_is_not_one() {
        // The directory holds whatever a package put there. A backup or a
        // disabled manifest keeps the driver's name and loses its extension,
        // and handing the loader one of those names it a file it cannot parse.
        assert!(!is_manifest_for("lvp_icd.x86_64.json.rpmsave", "lvp"));
        assert!(!is_manifest_for("lvp_icd.x86_64.json.disabled", "lvp"));
    }

    #[test]
    fn an_ordinary_run_names_the_software_reference_as_well_as_the_hardware() {
        // Not a preference but a requirement: the conformance suite renders the
        // same scene on the hardware driver and on the software one and
        // compares them. Naming only the first would trade a crash in the
        // loader for a test that silently stopped comparing anything, which is
        // the quieter of the two failures and therefore the worse.
        let Ok(env) = environment(Mode::Devices) else {
            return;
        };
        let Some((_, named)) = env.first() else {
            // No manifest this list recognizes, so nothing was named and the
            // loader scans as it always did. Nothing to assert about.
            return;
        };
        if lavapipe_icd().is_some() {
            assert!(
                named.contains("lvp"),
                "the software reference is installed and was not named: {named}"
            );
        }
    }
}
