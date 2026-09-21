//! What a device can actually do.
//!
//! Everything above the HAL branches on these values, never on backend
//! identity. A Mali GPU without `EGL_ANDROID_native_fence_sync` and a desktop
//! Vulkan driver without `VK_KHR_external_fence_fd` present the same problem,
//! and code that asks "is this GLES?" instead of "can this export a fence?"
//! gets both cases wrong.

use crate::format::FormatModifierSet;

/// Supported MSAA sample counts, as a bitmask of powers of two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SampleCounts(u32);

impl SampleCounts {
    pub const fn from_mask(mask: u32) -> Self {
        Self(mask)
    }

    /// Whether `count` is supported. Non-power-of-two counts are never valid.
    pub const fn supports(self, count: u32) -> bool {
        count.is_power_of_two() && (self.0 & count) != 0
    }

    /// The highest supported count, or 1 when only single-sampled rendering
    /// is available.
    pub const fn max(self) -> u32 {
        if self.0 == 0 {
            1
        } else {
            // Highest set bit.
            1 << (u32::BITS - 1 - self.0.leading_zeros())
        }
    }
}

/// How a device can move images across process or device boundaries.
///
/// Both halves matter independently. A render-only GPU paired with a separate
/// display controller — the common ARM SoC topology — needs export on one
/// device and import on the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DmaBufSupport {
    /// Can import a dma-buf as a texture.
    pub import: bool,
    /// Can export one of its own images as a dma-buf.
    pub export: bool,
    /// Can allocate and import with an explicit format modifier.
    ///
    /// Without this, the only safe shared layout is linear, and scanout gives
    /// up whatever bandwidth a vendor tiled or compressed layout would have
    /// saved.
    pub modifiers: bool,
}

impl DmaBufSupport {
    /// Whether this device can allocate its own scanout buffers.
    ///
    /// When false, the DRM presentation path must allocate through GBM and
    /// import instead. Both paths are supported; this is what picks between
    /// them.
    pub const fn can_allocate_scanout(self) -> bool {
        self.export && self.modifiers
    }
}

/// Whether GPU completion can be handed to another component as a fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncSupport {
    /// Can export a `sync_file` fd from a fence, for use as `IN_FENCE_FD` on
    /// an atomic commit.
    pub export_sync_file: bool,
    /// Can import a `sync_file` fd and wait on it on the GPU timeline.
    pub import_sync_file: bool,
}

impl SyncSupport {
    /// Whether the frame loop can stay fully explicit on the DRM path.
    ///
    /// When false, the DRM target must wait on the CPU before committing.
    /// That is correct but slower, and on Tier-1 hardware it is a driver bug
    /// to chase rather than a state to settle into, so callers are expected
    /// to log it loudly.
    pub const fn supports_explicit_scanout(self) -> bool {
        self.export_sync_file
    }
}

/// Everything the layers above the HAL are allowed to branch on.
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    /// Maximum width or height of a 2D texture.
    pub max_texture_size: u32,
    /// Supported MSAA sample counts for render targets.
    pub sample_counts: SampleCounts,
    /// Cross-device image sharing.
    pub dma_buf: DmaBufSupport,
    /// Cross-component synchronization.
    pub sync: SyncSupport,
    /// Whether the advanced blend modes are available.
    ///
    /// Both kinds: the separable ones from `Multiply` through `Exclusion` and
    /// the four non-separable ones. What they share is the thing that matters
    /// here -- [`BlendMode::factors`] has no pair to return for any of them --
    /// and what this does not gate is Porter-Duff, `Plus` or `Modulate`, which
    /// are factors and available everywhere.
    ///
    /// They need a hardware extension and cannot be emulated with blend
    /// factors, so a device without it refuses [`BlendMode::is_advanced`] modes
    /// rather than substituting the nearest expressible one. Callers check this
    /// before using one; nothing branches on which backend is in play.
    ///
    /// [`BlendMode::factors`]: crate::BlendMode::factors
    ///
    /// [`BlendMode::is_advanced`]: crate::BlendMode::is_advanced
    pub advanced_blend: bool,
    /// Whether a floating-point color attachment can be rendered into.
    ///
    /// A color outside the sRGB primaries' triangle has a component outside
    /// zero to one, and `Rgba16Float` is the only format here that can hold
    /// one. Whether a device will let it be a target is a separate question
    /// from whether it will sample one: half-float is filterable in core ES
    /// 3.0 and renderable only with an extension, and plenty of shipping
    /// drivers have the first and not the second. Vulkan answers per format
    /// from its format properties.
    ///
    /// Distinct from [`Self::render_formats`], which is the scanout list keyed
    /// by DRM fourcc and is empty on GLES. A format with no fourcc is
    /// deliberately absent from that list, so it cannot answer this.
    pub float_render_targets: bool,
    /// Formats and layouts this device can render into and export.
    ///
    /// One half of format negotiation; the presentation target supplies the
    /// other.
    pub render_formats: Vec<FormatModifierSet>,
    /// Human-readable device and driver identification, for report
    /// fingerprints and bug reports.
    pub device_name: String,
    pub driver_name: String,
    /// Whether rendering happens on the CPU rather than on a GPU.
    ///
    /// Not a performance hint. It marks the device properties that are
    /// consequences of having no graphics hardware rather than defects: a CPU
    /// rasterizer has no tiling to describe, so advertising only a linear
    /// layout is the correct answer for it and a sign of a missing modifier
    /// query on anything else. A test that cannot tell those apart has to
    /// choose between failing on software and not checking hardware, and both
    /// are worse than asking.
    ///
    /// Vulkan takes this from the device type, which is authoritative. GLES has
    /// no equivalent query and it is recognized from the renderer string, which
    /// is not; a software implementation this does not know the name of reports
    /// false, so treat a true as reliable and a false as merely unremarkable.
    pub software: bool,
}

impl Capabilities {
    /// Whether this device can drive a KMS plane directly, by either
    /// allocation strategy.
    ///
    /// Note this says nothing about sync: a device can be scanout-capable and
    /// still lack fence export, in which case the path works with a CPU wait.
    pub fn supports_scanout(&self) -> bool {
        self.dma_buf.can_allocate_scanout() || self.dma_buf.import
    }

    /// Whether every blend mode a batch uses is available on this device.
    ///
    /// Lives here rather than in each backend so the two refuse the same batch
    /// for the same reason: a Vulkan device without the advanced-blend
    /// extension and a GLES context without it are the same problem, and a
    /// check written twice is a check that eventually disagrees with itself.
    /// Refusing is deliberate — the alternative is substituting the nearest
    /// expressible mode, which produces a picture nobody can debug from.
    pub fn check_blend_modes(&self, batch: &crate::Batch) -> crate::Result<()> {
        if !self.advanced_blend && batch.draws().iter().any(|draw| draw.blend.is_advanced()) {
            return Err(crate::Error::Unsupported(
                "advanced blend modes; this device has no advanced-blend extension",
            ));
        }
        Ok(())
    }

    /// Whether this device can create the texture a descriptor asks for.
    ///
    /// Here rather than in each backend for the reason
    /// [`Self::check_blend_modes`] is: two backends refusing the same thing for
    /// the same reason, in one place, rather than a check written twice that
    /// eventually disagrees with itself. Without it the failure arrives as a
    /// framebuffer-incomplete number on one backend and a driver error on the
    /// other, neither of which names what was missing.
    pub fn check_texture(&self, desc: &crate::TextureDescriptor) -> crate::Result<()> {
        // Refused for every device rather than for some, because this is not a
        // capability: the pipeline carries sRGB-encoded components, so a target
        // that applies the transfer on write applies it twice and the picture
        // comes back over a third too bright. See `PixelFormat::is_drawable`.
        //
        // Only a render target. Sampling one decodes, which is also not what an
        // encoded pipeline wants, but a caller may have data this is right for
        // and `Context::create_image` says which format a picture wants. Drawing
        // into one has no reading under which it is correct.
        if desc.usage.render_target && !desc.format.is_drawable() {
            return Err(crate::Error::Unsupported(
                "an sRGB render target; this pipeline already holds encoded \
                 color and the format would encode it a second time",
            ));
        }
        if desc.usage.render_target
            && desc.format == crate::PixelFormat::Rgba16Float
            && !self.float_render_targets
        {
            return Err(crate::Error::Unsupported(
                "a floating-point render target; this device can sample one but not draw into it",
            ));
        }
        Ok(())
    }

    /// Whether an extent fits within the device's texture limit.
    pub fn can_allocate(&self, extent: crate::format::Extent2D) -> bool {
        extent.width <= self.max_texture_size && extent.height <= self.max_texture_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Extent2D;

    /// The advanced-blend refusal, which had no test while the texture refusal
    /// beside it did.
    ///
    /// Three cases rather than one, because a check that refuses is only
    /// correct if it also lets things through: a refusal that fired on every
    /// batch would pass a test asserting only that an advanced mode is refused,
    /// and this renderer draws almost nothing in an advanced mode.
    ///
    /// This is the whole of what a device lacking the extension gets. There is
    /// no second implementation to fall back to -- upstream has one, reached
    /// through framebuffer fetch, and the comment above says why substituting
    /// is refused instead. So a caller asking for `Multiply` on such a device
    /// gets this error and never a different picture.
    #[test]
    fn an_advanced_mode_is_refused_only_where_the_extension_is_missing() {
        const TRI: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];

        let batch = |blend| {
            let mut batch = crate::Batch::new();
            batch
                .push(&TRI, &[0, 1, 2], crate::Material::solid([1.0; 4]), blend)
                .expect("one triangle is within any batch's limits");
            batch
        };
        let advanced = batch(crate::BlendMode::Multiply);
        let ordinary = batch(crate::BlendMode::SrcOver);
        assert!(
            crate::BlendMode::Multiply.is_advanced() && !crate::BlendMode::SrcOver.is_advanced(),
            "the two modes chosen here have to sit on opposite sides of the \
             thing being checked, or this test checks nothing"
        );

        let without = Capabilities {
            advanced_blend: false,
            ..Default::default()
        };
        let with = Capabilities {
            advanced_blend: true,
            ..Default::default()
        };

        assert!(
            without.check_blend_modes(&advanced).is_err(),
            "an advanced mode was allowed on a device with no advanced-blend \
             extension, which reaches the driver as a mode it cannot express"
        );
        assert!(
            with.check_blend_modes(&advanced).is_ok(),
            "an advanced mode was refused on a device that has the extension"
        );
        assert!(
            without.check_blend_modes(&ordinary).is_ok(),
            "an ordinary mode was refused for want of an extension it does not \
             need -- the check is looking at the batch rather than at the mode"
        );
    }

    #[test]
    fn sample_counts_reject_non_powers_of_two() {
        let counts = SampleCounts::from_mask(1 | 2 | 4);
        assert!(counts.supports(1));
        assert!(counts.supports(4));
        assert!(!counts.supports(8));
        // 3 has bits in common with the mask but is not a valid count.
        assert!(!counts.supports(3));
    }

    #[test]
    fn sample_counts_report_max_and_degrade_to_single_sampled() {
        assert_eq!(SampleCounts::from_mask(1 | 2 | 4).max(), 4);
        assert_eq!(SampleCounts::default().max(), 1);
    }

    #[test]
    fn self_allocation_requires_both_export_and_modifiers() {
        let full = DmaBufSupport {
            import: true,
            export: true,
            modifiers: true,
        };
        assert!(full.can_allocate_scanout());

        // Export without modifier support cannot negotiate a scanout-capable
        // layout, so allocation has to go through GBM instead.
        let no_modifiers = DmaBufSupport {
            modifiers: false,
            ..full
        };
        assert!(!no_modifiers.can_allocate_scanout());
    }

    #[test]
    fn import_only_devices_still_support_scanout() {
        // The GBM-allocated path: cannot allocate its own scanout buffers but
        // can import ones GBM made.
        let caps = Capabilities {
            dma_buf: DmaBufSupport {
                import: true,
                export: false,
                modifiers: false,
            },
            ..Default::default()
        };
        assert!(!caps.dma_buf.can_allocate_scanout());
        assert!(caps.supports_scanout());
    }

    #[test]
    fn scanout_capability_is_independent_of_fence_export() {
        let caps = Capabilities {
            dma_buf: DmaBufSupport {
                import: true,
                export: true,
                modifiers: true,
            },
            sync: SyncSupport::default(),
            ..Default::default()
        };
        // Scanout works; it just cannot stay explicit, which is the caller's
        // cue to log the CPU-wait fallback rather than to disable the path.
        assert!(caps.supports_scanout());
        assert!(!caps.sync.supports_explicit_scanout());
    }

    #[test]
    fn allocation_limit_is_checked_on_both_axes() {
        let caps = Capabilities {
            max_texture_size: 4096,
            ..Default::default()
        };
        assert!(caps.can_allocate(Extent2D::new(4096, 4096)));
        assert!(!caps.can_allocate(Extent2D::new(4097, 16)));
        assert!(!caps.can_allocate(Extent2D::new(16, 4097)));
    }

    /// Sampling a half-float texture and drawing into one are separate
    /// permissions, and a device may offer the first without the second.
    #[test]
    fn a_float_target_is_refused_where_only_sampling_one_is_offered() {
        let mut caps = Capabilities {
            float_render_targets: false,
            ..Capabilities::default()
        };
        let extent = Extent2D::new(4, 4);

        let target = crate::TextureDescriptor::offscreen(extent, crate::PixelFormat::Rgba16Float);
        assert!(
            caps.check_texture(&target).is_err(),
            "a float attachment was allowed where the device offers none"
        );

        // Sampling one is a different request and is not refused, which is the
        // whole reason the two are separate: a gradient ramp wants exactly this
        // and would otherwise be unavailable on the same devices.
        let sampled = crate::TextureDescriptor::sampled(extent, crate::PixelFormat::Rgba16Float);
        assert!(caps.check_texture(&sampled).is_ok());

        // And an eight-bit target is never the question.
        let ordinary = crate::TextureDescriptor::offscreen(extent, crate::PixelFormat::Rgba8Unorm);
        assert!(caps.check_texture(&ordinary).is_ok());

        caps.float_render_targets = true;
        assert!(caps.check_texture(&target).is_ok());
    }
}
