//! Shader sources and the artifacts built from them.
//!
//! One WGSL source tree is the single source of truth. `build.rs` runs naga
//! over it to produce SPIR-V for Vulkan, and can emit GLSL, MSL, and HLSL for
//! backends that do not exist yet, from the same source and with no C++ shader
//! toolchain anywhere in the build.
//!
//! Translation happens at build time, so nothing here parses shaders at
//! runtime and no shader compiler ships in the binary. That is what makes
//! "all pipelines compiled ahead of time, no compilation jank" achievable
//! rather than aspirational.

include!(concat!(env!("OUT_DIR"), "/shaders.rs"));

#[cfg(test)]
mod tests {
    #[test]
    fn spirv_is_well_formed() {
        // The SPIR-V magic number. A mismatch means the generator emitted
        // something other than a module, or emitted it byte-swapped.
        assert_eq!(super::SOLID_SPV.first().copied(), Some(0x0723_0203));
        // Header is five words; anything at or below that is an empty module.
        assert!(super::SOLID_SPV.len() > 5, "module is header-only");
    }
}
