//! Device bring-up against whatever drivers this machine has.
//!
//! These tests need a Vulkan loader and at least one device. Where neither
//! exists they skip rather than fail: a developer without a GPU stack should
//! still get a green `cargo test`, and the lanes that must have Vulkan enforce
//! it by running on images that provide it.

use impeller_hal_vulkan::{DevicePreference, VulkanContext};

/// Create a context, or `None` if this machine has no usable Vulkan.
fn context(preference: DevicePreference) -> Option<VulkanContext> {
    match VulkanContext::new(preference) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable Vulkan device ({e})");
            None
        }
    }
}

#[test]
fn a_context_reports_coherent_capabilities() {
    let Some(ctx) = context(DevicePreference::Auto) else {
        return;
    };
    let caps = ctx.capabilities();
    eprintln!("device: {} ({})", caps.device_name, caps.driver_name);

    assert!(
        !caps.device_name.is_empty(),
        "a device that cannot name itself makes every report fingerprint useless"
    );
    assert!(
        caps.max_texture_size >= 4096,
        "got {}",
        caps.max_texture_size
    );

    // Single-sampled rendering is universally supported; a device reporting
    // otherwise has a mask decoded wrong.
    assert!(caps.sample_counts.supports(1));
    assert!(caps.sample_counts.max() >= 1);
    assert!(caps.sample_counts.max().is_power_of_two());
}

#[test]
fn capability_flags_are_internally_consistent() {
    let Some(ctx) = context(DevicePreference::Auto) else {
        return;
    };
    let caps = ctx.capabilities();

    // Self-allocated scanout requires both export and explicit modifiers.
    // Reporting the composite true while a component is false would send the
    // presentation layer down a path the driver cannot support.
    if caps.dma_buf.can_allocate_scanout() {
        assert!(caps.dma_buf.export);
        assert!(caps.dma_buf.modifiers);
    }

    // The extension backing each flag must actually have been enabled, or the
    // flag is a promise the device cannot keep.
    // Export is a semaphore operation and import is a fence one, so the two
    // flags come from different extensions rather than one covering both.
    assert_eq!(
        caps.sync.export_sync_file,
        ctx.has_extension("VK_KHR_external_semaphore_fd")
    );
    assert_eq!(
        caps.sync.import_sync_file,
        ctx.has_extension("VK_KHR_external_fence_fd")
    );
    assert_eq!(
        caps.dma_buf.modifiers,
        ctx.has_extension("VK_EXT_image_drm_format_modifier")
    );
}

#[test]
fn scanout_and_explicit_sync_are_reported_independently() {
    let Some(ctx) = context(DevicePreference::Auto) else {
        return;
    };
    let caps = ctx.capabilities();
    eprintln!(
        "scanout={} explicit_sync={} import={} export={} modifiers={}",
        caps.supports_scanout(),
        caps.sync.supports_explicit_scanout(),
        caps.dma_buf.import,
        caps.dma_buf.export,
        caps.dma_buf.modifiers,
    );

    // A device can be scanout-capable without fence export; that combination
    // selects the CPU-wait fallback rather than disabling the DRM path, so
    // neither flag may be derived from the other.
    if !caps.supports_scanout() {
        assert!(!caps.dma_buf.can_allocate_scanout());
    }
}

#[test]
fn the_software_rasterizer_is_selectable_as_the_reference_device() {
    match VulkanContext::new(DevicePreference::Software) {
        Ok(ctx) => {
            let caps = ctx.capabilities();
            eprintln!("software device: {}", caps.device_name);
            // The golden corpus is compared against this device, so it has to
            // be usable for real rendering, not merely enumerable.
            assert!(caps.max_texture_size >= 4096);
            assert!(caps.sample_counts.supports(1));
        }
        Err(e) => eprintln!("skipping: no software rasterizer ({e})"),
    }
}

#[test]
fn an_out_of_range_device_index_is_an_error_rather_than_a_panic() {
    // Device indices come from configuration and command lines, so an invalid
    // one must surface as an error the caller can report.
    let result = VulkanContext::new(DevicePreference::Index(9999));
    assert!(result.is_err());
}

#[test]
fn contexts_can_be_created_and_dropped_repeatedly() {
    if context(DevicePreference::Auto).is_none() {
        return;
    }
    // Teardown order is instance-after-device; getting it wrong tends to show
    // up as a crash on the second cycle rather than the first.
    for _ in 0..3 {
        let ctx = VulkanContext::new(DevicePreference::Auto).expect("device was available");
        assert!(!ctx.capabilities().device_name.is_empty());
    }
}

#[test]
fn enabled_extensions_carry_their_dependencies() {
    let Some(ctx) = context(DevicePreference::Auto) else {
        return;
    };

    // The specification requires every dependency of an enabled extension to
    // be enabled too, and a device created without them is invalid even though
    // it appears to work. This only shows up under a validation layer, so the
    // invariant is asserted directly here instead.
    if ctx.has_extension("VK_EXT_image_drm_format_modifier") {
        assert!(
            ctx.has_extension("VK_KHR_image_format_list"),
            "the modifier extension requires VK_KHR_image_format_list on a 1.1 baseline"
        );
    }
    if ctx.has_extension("VK_EXT_external_memory_dma_buf") {
        assert!(
            ctx.has_extension("VK_KHR_external_memory_fd"),
            "dma-buf external memory requires the fd extension"
        );
    }
}
