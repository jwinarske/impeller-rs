//! Validation is asserted, not eyeballed.
//!
//! A device created invalid can pass an entire suite while appearing to work,
//! so these tests turn the layer on and require the log to come back empty.
//! They skip where the layer is not installed, since a machine without the SDK
//! should still get a green run.

use impeller_hal::{Extent2D, PixelFormat, TextureDescriptor};
use impeller_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};

fn validated(device: DevicePreference) -> Option<VulkanContext> {
    let ctx = VulkanContext::with_config(ContextConfig {
        device,
        validation: true,
    })
    .ok()?;
    if !ctx.validation_active() {
        eprintln!("skipping: validation layer not installed");
        return None;
    }
    Some(ctx)
}

fn assert_clean(ctx: &VulkanContext, what: &str) {
    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == impeller_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "{what} produced {} validation error(s):\n{}",
        errors.len(),
        errors
            .iter()
            .map(|e| format!("  [{}] {}", e.id, e.message))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn device_creation_is_validation_clean() {
    let Some(ctx) = validated(DevicePreference::Auto) else {
        return;
    };
    // This is the test that would have caught the missing image-format-list
    // dependency, which every pixel test passed straight through.
    assert_clean(&ctx, "device creation");
    assert!(ctx.validation_clean());
}

#[test]
fn allocate_clear_and_read_is_validation_clean() {
    let Some(mut ctx) = validated(DevicePreference::Auto) else {
        return;
    };
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(32, 32),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");
    ctx.clear_texture(&mut tex, [0.0, 1.0, 1.0, 1.0])
        .expect("clear");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);

    assert_eq!(&pixels[..4], &[0, 255, 255, 255]);
    // Layout transitions and copy regions are exactly what the layer checks,
    // so a clean log here is worth more than the pixel assertion above.
    assert_clean(&ctx, "allocate, clear and read");
}

#[test]
fn the_software_reference_is_validation_clean() {
    let Some(mut ctx) = validated(DevicePreference::Software) else {
        return;
    };
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(16, 16),
            PixelFormat::Bgra8Unorm,
        ))
        .expect("texture");
    ctx.clear_texture(&mut tex, [1.0, 0.0, 0.0, 1.0])
        .expect("clear");
    ctx.destroy_texture(tex);
    assert_clean(&ctx, "software reference");
}

#[test]
fn repeated_context_cycles_stay_clean() {
    if validated(DevicePreference::Auto).is_none() {
        return;
    }
    // Teardown ordering bugs tend to report on the second or third cycle.
    for i in 0..3 {
        let ctx = validated(DevicePreference::Auto).expect("layer was present");
        assert_clean(&ctx, &format!("cycle {i}"));
    }
}
