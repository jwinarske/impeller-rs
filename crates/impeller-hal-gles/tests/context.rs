//! GLES context bring-up against whatever EGL this machine provides.
//!
//! Skips where EGL or a surfaceless platform is unavailable, so a machine
//! without a graphics stack still gets a green run, while lanes that must have
//! GLES enforce it through the image they run on.

use impeller_hal_gles::DisplayTarget;
use impeller_hal_gles::Validated as GlesValidated;

fn context() -> Option<GlesValidated> {
    match GlesValidated::new(DisplayTarget::Surfaceless) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable GLES context ({e})");
            None
        }
    }
}

#[test]
fn a_context_reports_coherent_capabilities() {
    let Some(ctx) = context() else { return };
    let caps = ctx.capabilities();
    eprintln!("device: {} ({})", caps.device_name, caps.driver_name);

    assert!(
        !caps.device_name.is_empty(),
        "a device that cannot name itself makes every report fingerprint useless"
    );
    // GLES 3.0 guarantees at least 2048; anything less means the context came
    // up below the floor this project targets.
    assert!(
        caps.max_texture_size >= 2048,
        "got {}",
        caps.max_texture_size
    );
    assert!(caps.sample_counts.supports(1));
    assert!(caps.sample_counts.max().is_power_of_two());
}

#[test]
fn the_context_is_at_least_gles_three() {
    let Some(ctx) = context() else { return };
    let version = &ctx.capabilities().driver_name;
    // GLES 2.0 is permanently out of scope, so a 2.x context is a failure to
    // start rather than a degraded mode.
    assert!(
        version.contains("OpenGL ES 3"),
        "expected an ES 3 context, got {version}"
    );
}

#[test]
fn sample_counts_are_powers_of_two_and_may_have_gaps() {
    let Some(ctx) = context() else { return };
    let counts = ctx.capabilities().sample_counts;
    // This used to require a contiguous run, on the belief that GLES reports
    // only a maximum and that the mask below it is safe to synthesize. It is
    // not: `GetInternalformativ` reports the counts a format actually supports,
    // and llvmpipe answers 8 and 4 for `RGBA8` with no 2. So a gap is a real
    // device answer rather than a broken query, and requiring contiguity meant
    // requiring the wrong thing on a conformant driver.
    //
    // What remains true is the shape. Anything else -- which counts, how many
    // -- is the device's to say, and pinning it here would only re-encode the
    // assumption this replaced.
    let max = counts.max();
    assert!(
        counts.supports(1),
        "single-sampled is always available and was not reported"
    );
    assert!(max.is_power_of_two(), "maximum {max} is not a power of two");
    for n in [1u32, 2, 4, 8, 16, 32] {
        if counts.supports(n) {
            assert!(n <= max, "{n}x is supported but above the maximum {max}");
        }
    }
    assert!(!counts.supports(max * 2), "{}x above the maximum", max * 2);
}

#[test]
fn capability_flags_match_the_extensions_actually_present() {
    let Some(ctx) = context() else { return };
    let caps = ctx.capabilities();

    // Import and export are separate extensions here, unlike Vulkan where one
    // handle type covers both. A flag that did not match its extension would be
    // a promise the driver cannot keep.
    assert_eq!(
        caps.dma_buf.import,
        ctx.has_egl_extension("EGL_EXT_image_dma_buf_import")
    );
    assert_eq!(
        caps.dma_buf.export,
        ctx.has_egl_extension("EGL_MESA_image_dma_buf_export")
    );
    assert_eq!(
        caps.sync.export_sync_file,
        ctx.has_egl_extension("EGL_ANDROID_native_fence_sync")
    );

    eprintln!(
        "scanout={} explicit_sync={} import={} export={} modifiers={}",
        caps.supports_scanout(),
        caps.sync.supports_explicit_scanout(),
        caps.dma_buf.import,
        caps.dma_buf.export,
        caps.dma_buf.modifiers,
    );
}

#[test]
fn gl_calls_work_on_the_current_context() {
    use glow::HasContext;
    let Some(ctx) = context() else { return };
    let gl = ctx.raw_gl();

    // Proves the loader resolved real entry points rather than nulls, which
    // would otherwise surface as a crash at the first draw rather than here.
    // SAFETY: the context is current on this thread for its whole lifetime.
    unsafe {
        assert_eq!(gl.get_error(), glow::NO_ERROR);
        let viewport = gl.get_parameter_i32(glow::MAX_VIEWPORT_DIMS);
        assert!(
            viewport > 0,
            "max viewport dimension came back as {viewport}"
        );
        assert_eq!(gl.get_error(), glow::NO_ERROR);
    }
}

#[test]
fn contexts_can_be_created_and_dropped_repeatedly() {
    if context().is_none() {
        return;
    }
    // Teardown unbinds before destroying; getting that wrong tends to leak the
    // display and show up on a later cycle rather than the first.
    for i in 0..3 {
        let ctx = GlesValidated::new(DisplayTarget::Surfaceless)
            .unwrap_or_else(|e| panic!("cycle {i}: {e}"));
        assert!(!ctx.capabilities().device_name.is_empty());
    }
}
