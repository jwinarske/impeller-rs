//! Shader sources and build-time translation.
//!
//! One WGSL source tree is the single source of truth; naga emits SPIR-V for
//! Vulkan and GLSL ES 300 for GLES, with MSL, HLSL, and desktop GLSL available
//! for future backends from the same source and no C++ shader toolchain.
//!
//! Hand-written overrides live alongside, and the build fails if an override
//! goes stale against its WGSL twin. naga output for every shader and target is
//! snapshotted and diffed in CI, so a naga upgrade is a reviewed event rather
//! than a silent behavior change.
