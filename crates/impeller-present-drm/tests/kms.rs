//! The scanout path against a real KMS device.
//!
//! Everything the recording stand-in cannot answer: whether a display
//! controller accepts the buffers this renderer exports, whether an atomic
//! commit with those properties is valid, and whether the flip events come back.
//!
//! It needs a card this process can become master of. A compositor holds
//! master on any card driving a display, so in practice that means the virtual
//! KMS driver — which provides atomic modesetting and vblank with no display
//! behind it. `cargo xtask drm` says whether a machine has one; where it does
//! not, these skip and say so rather than passing quietly.

use impeller_hal::{Batch, BlendMode, HalContext, Material, PassDescriptor, PixelFormat};
use impeller_hal_vulkan::{DevicePreference, VulkanContext};
use impeller_present::negotiate::negotiate;
use impeller_present_drm::output::{CommitRequest, DmaBufPlanes, OutputEvent, ScanoutOutput};
use impeller_present_drm::KmsOutput;

const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// A card this process can drive, or nothing.
fn output() -> Option<KmsOutput> {
    let mut refused = Vec::new();
    for entry in std::fs::read_dir("/dev/dri").ok()?.flatten() {
        let name = entry.file_name().into_string().ok()?;
        if !name.starts_with("card") {
            continue;
        }
        let path = entry.path().to_string_lossy().into_owned();
        match KmsOutput::open(&path) {
            Ok(output) => return Some(output),
            Err(e) => refused.push(format!("{path}: {e}")),
        }
    }
    eprintln!(
        "skipping: no card this process can drive ({})",
        refused.join("; ")
    );
    None
}

fn context() -> Option<VulkanContext> {
    match VulkanContext::new(DevicePreference::Auto) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no Vulkan device ({e})");
            None
        }
    }
}

#[test]
fn a_rendered_frame_reaches_a_real_display_controller() {
    let (Some(mut output), Some(mut ctx)) = (output(), context()) else {
        return;
    };
    let mode = output.mode();
    eprintln!(
        "driving {} at {}x{}",
        output.path(),
        mode.extent.width,
        mode.extent.height
    );

    // Negotiation against what this plane advertises, rather than a layout
    // chosen in advance. Linear is a valid answer here: vkms accepts nothing
    // else, and that being reported rather than assumed is the point.
    let render = HalContext::capabilities(&ctx).render_formats.clone();
    let agreed = negotiate(
        &render,
        output.supported_formats(),
        &[
            impeller_hal::Fourcc::ARGB8888,
            impeller_hal::Fourcc::XRGB8888,
            impeller_hal::Fourcc::ABGR8888,
        ],
    )
    .expect("the render device and this plane share no layout");
    eprintln!("agreed on {:?} with {:?}", agreed.fourcc, agreed.modifier);

    // Allocate an image the display can scan out, draw into it, and export it.
    let format = PixelFormat::Bgra8Unorm;
    let mut target = ctx
        .create_exportable_texture(mode.extent, format, &[agreed.modifier])
        .expect("an exportable image at the negotiated layout");

    let mut batch = Batch::new();
    batch
        .push(
            &FULL,
            &QUAD,
            Material::solid([0.2, 0.6, 0.9, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    ctx.submit_batch(&mut target, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("render into the scanout buffer");

    let exported = ctx.export_texture(&target).expect("dma-buf export");
    let planes = DmaBufPlanes {
        planes: exported.planes,
        fourcc: agreed.fourcc,
        modifier: agreed.modifier,
        extent: mode.extent,
    };

    // The whole point: a buffer this renderer allocated, drew into and
    // exported, accepted by a display controller as a framebuffer.
    let fb = output
        .import_dmabuf(planes)
        .expect("the display controller refused the exported buffer");

    // First commit sets the mode; a flip alone would have nothing to flip onto.
    output
        .commit(CommitRequest {
            fb,
            in_fence_fd: None,
            allow_modeset: true,
        })
        .expect("atomic commit");

    // vkms drives vblank off a timer, so the flip lands within a frame. Waiting
    // rather than polling once is what distinguishes an event that arrives from
    // one that never does.
    let events = output
        .wait_for_event(500_000_000)
        .expect("waiting for the flip");
    assert!(
        events.contains(&OutputEvent::FlipComplete { fb }),
        "no flip-complete arrived for the committed frame: {events:?}"
    );

    output.release_framebuffer(fb);
    ctx.destroy_texture(target);
}

#[test]
fn several_frames_flip_in_turn() {
    let (Some(mut output), Some(mut ctx)) = (output(), context()) else {
        return;
    };
    let mode = output.mode();
    let agreed = negotiate(
        &HalContext::capabilities(&ctx).render_formats.clone(),
        output.supported_formats(),
        &[
            impeller_hal::Fourcc::ARGB8888,
            impeller_hal::Fourcc::XRGB8888,
        ],
    )
    .expect("negotiation");

    // Two buffers, alternating, which is the arrangement a frame loop uses: one
    // on screen while the next is drawn. A single buffer would be drawn into
    // while the controller was reading it.
    let mut slots = Vec::new();
    for _ in 0..2 {
        let mut texture = ctx
            .create_exportable_texture(mode.extent, PixelFormat::Bgra8Unorm, &[agreed.modifier])
            .expect("exportable image");
        let mut batch = Batch::new();
        batch
            .push(
                &FULL,
                &QUAD,
                Material::solid([0.9, 0.3, 0.1, 1.0]),
                BlendMode::Src,
            )
            .expect("push");
        ctx.submit_batch(&mut texture, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("render");
        let exported = ctx.export_texture(&texture).expect("export");
        let fb = output
            .import_dmabuf(DmaBufPlanes {
                planes: exported.planes,
                fourcc: agreed.fourcc,
                modifier: agreed.modifier,
                extent: mode.extent,
            })
            .expect("import");
        slots.push((texture, fb));
    }

    for frame in 0..6 {
        let (_, fb) = slots[frame % slots.len()];
        output
            .commit(CommitRequest {
                fb,
                in_fence_fd: None,
                // Only the first commit may modeset; asking every frame turns a
                // flip into something far more expensive.
                allow_modeset: frame == 0,
            })
            .unwrap_or_else(|e| panic!("frame {frame}: {e}"));
        let events = output
            .wait_for_event(500_000_000)
            .unwrap_or_else(|e| panic!("frame {frame}: {e}"));
        assert!(
            events.contains(&OutputEvent::FlipComplete { fb }),
            "frame {frame} never flipped: {events:?}"
        );
    }

    for (texture, fb) in slots {
        output.release_framebuffer(fb);
        ctx.destroy_texture(texture);
    }
}

#[test]
fn committing_a_framebuffer_this_output_did_not_import_is_refused() {
    let Some(mut output) = output() else { return };
    // A handle from somewhere else names a framebuffer the kernel would reject
    // or, worse, one belonging to another import. Refusing before the commit
    // keeps that from being an atomic failure nobody can read.
    let result = output.commit(CommitRequest {
        fb: impeller_present_drm::FbHandle(0xdead_beef),
        in_fence_fd: None,
        allow_modeset: false,
    });
    assert!(result.is_err(), "an unknown framebuffer was committed");
}

#[test]
fn the_scanout_target_drives_a_real_display_controller() {
    // The whole stack, with nothing standing in: the presentation target's ring
    // and fence accounting, driving a real KMS device, rendering with the HAL.
    // Every test above this one exercises a piece; this is the one that says
    // the pieces fit.
    use impeller_hal_vulkan::VulkanHal;
    use impeller_present::PresentTarget;
    use impeller_present_drm::DrmScanoutTarget;

    let (Some(output), Some(mut ctx)) = (output(), context()) else {
        return;
    };
    let mode = output.mode();
    eprintln!(
        "{}x{} at {} mHz",
        mode.extent.width, mode.extent.height, mode.refresh_mhz
    );

    // Three buffers: one on screen, one being flipped to, one being drawn.
    let mut target = match DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 3) {
        Ok(target) => target,
        Err(e) => panic!("building the scanout target: {e}"),
    };

    for frame in 0..8u32 {
        let image = target.acquire(&mut ctx).expect("acquire");
        let mut batch = Batch::new();
        let t = frame as f32 / 8.0;
        batch
            .push(
                &FULL,
                &QUAD,
                Material::solid([t, 0.4, 1.0 - t, 1.0]),
                BlendMode::Src,
            )
            .expect("push");
        // Deferred every frame, so every commit has a fence to hand the
        // kernel. The target withholds it on the one commit that also
        // modesets, which is the case vkms never completes a flip for.
        let fence = ctx
            .submit_batch_deferred(image, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("deferred submission");
        target
            .set_frame_fence(fence)
            .expect("attach the render fence");
        target
            .present(&mut ctx)
            .unwrap_or_else(|e| panic!("frame {frame}: {e}"));
    }

    // Every frame reached the controller, and the ring did not stall waiting
    // for one that never flipped. The CPU-wait count is what says whether the
    // fence rode the commit or the frame loop had to block first.
    // Exactly one: the first frame, whose commit also set the mode. Every
    // frame after it handed its fence to the kernel and blocked on nothing,
    // which is the property the DRM path exists for and the one a count of
    // zero-or-eight would fail to distinguish.
    assert_eq!(
        target.cpu_waits(),
        1,
        "expected a stall only on the modesetting frame"
    );
    eprintln!("ring depth {}", target.ring_depth());
    target.destroy(&mut ctx);
}
