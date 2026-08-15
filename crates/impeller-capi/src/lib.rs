//! Impeller C API: an ABI-compatible implementation of upstream's
//! `impeller.h`.
//!
//! This crate builds `libimpeller`, so a consumer that links against the
//! upstream C API can link against this instead without recompiling. The
//! target is binary compatibility with the header, not a Rust-flavored
//! approximation of it.
//!
//! # What this is not
//!
//! It is **not** a drop-in for Impeller inside the Flutter Engine build. The
//! engine does not consume Impeller across this boundary — it compiles the C++
//! sources directly, against internal C++ classes with templates, STL types,
//! and virtual inheritance that no Rust library can present. This crate serves
//! the embedder-facing C API only.
//!
//! # Conventions the header imposes
//!
//! - **Reference counting.** Every handle type has `Retain` and `Release`
//!   entry points, and both are no-ops when passed NULL.
//! - **No error channel.** Creation functions return NULL on failure and
//!   fallible operations return `bool`. There are no error codes and no way to
//!   report a reason, so anything diagnostic has to go to the log.
//! - **Version negotiation.** The caller passes its compiled-in version to
//!   context creation, and a mismatch fails the call. See [`IMPELLER_VERSION`].
//!
//! # Verifying parity
//!
//! Symbol-level parity is checked against a pinned copy of the upstream
//! header rather than maintained by hand: CI diffs our exported symbols
//! against the header's declarations, so an upstream addition shows up as a
//! failure naming the missing entry points. Semantic parity — blend mode
//! values, fill rules, color handling, stroke geometry — is checked by running
//! the upstream C API samples against this library and comparing output
//! through the usual golden comparators.
//!
//! # Status
//!
//! Only version negotiation is implemented. The rest of the surface waits on
//! the renderer; stubbing entry points that return NULL would be worse than
//! their absence, because a consumer would link successfully and then fail at
//! runtime with no diagnostic.

#![allow(non_snake_case)]

/// Packs a version the way the header's `IMPELLER_MAKE_VERSION` does.
pub const fn make_version(variant: u32, major: u32, minor: u32, patch: u32) -> u32 {
    (variant << 29) | (major << 22) | (minor << 12) | patch
}

pub const fn version_variant(v: u32) -> u32 {
    v >> 29
}

pub const fn version_major(v: u32) -> u32 {
    (v >> 22) & 0x7f
}

pub const fn version_minor(v: u32) -> u32 {
    (v >> 12) & 0x3ff
}

pub const fn version_patch(v: u32) -> u32 {
    v & 0xfff
}

/// The API version this library implements.
///
/// Must track the pinned upstream header exactly. A consumer compiled against
/// a different version is refused at context creation rather than allowed to
/// run against a surface it may not match.
pub const IMPELLER_VERSION: u32 = make_version(1, 1, 4, 0);

/// `ImpellerGetVersion` — the version this library implements.
///
/// Callers use this to check compatibility before creating a context.
#[no_mangle]
pub extern "C" fn ImpellerGetVersion() -> u32 {
    IMPELLER_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_packing_round_trips() {
        let v = make_version(1, 1, 4, 0);
        assert_eq!(version_variant(v), 1);
        assert_eq!(version_major(v), 1);
        assert_eq!(version_minor(v), 4);
        assert_eq!(version_patch(v), 0);
    }

    #[test]
    fn version_fields_do_not_bleed_into_each_other() {
        // Maximum values per field, to catch a shift or mask that overlaps a
        // neighbor. A packing bug here would make version negotiation accept
        // or reject the wrong callers.
        let v = make_version(0b111, 0x7f, 0x3ff, 0xfff);
        assert_eq!(version_variant(v), 0b111);
        assert_eq!(version_major(v), 0x7f);
        assert_eq!(version_minor(v), 0x3ff);
        assert_eq!(version_patch(v), 0xfff);
    }

    #[test]
    fn exported_version_matches_the_constant() {
        assert_eq!(ImpellerGetVersion(), IMPELLER_VERSION);
    }
}
