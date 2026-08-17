//! Exporting GPU completion as a sync_file.
//!
//! The second of the two requirements the HAL reserved from day one. The DRM
//! presentation path attaches this descriptor to an atomic commit so the kernel
//! latches the page flip when rendering finishes, instead of the CPU waiting
//! and then committing. Without it the frame loop blocks in the middle, which
//! is a full frame of latency rather than a correctness problem — the kind of
//! regression that is easy to introduce and hard to notice.

use impeller_hal::{Batch, BlendMode, Extent2D, HalFence, PassDescriptor, PixelFormat};
use impeller_hal::{TextureDescriptor, FRAME_WAIT_TIMEOUT};
use impeller_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};

const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

fn context() -> Option<VulkanContext> {
    match VulkanContext::with_config(ContextConfig {
        device: DevicePreference::Auto,
        validation: true,
    }) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable Vulkan device ({e})");
            None
        }
    }
}

fn scene() -> Batch {
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, [1.0, 0.0, 0.0, 1.0], BlendMode::Src)
        .expect("push");
    batch
}

fn target(ctx: &mut VulkanContext) -> impeller_hal_vulkan::VulkanTexture {
    ctx.create_texture(&TextureDescriptor::offscreen(
        Extent2D::new(64, 64),
        PixelFormat::Rgba8Unorm,
    ))
    .expect("texture")
}

fn assert_clean(ctx: &VulkanContext) {
    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == impeller_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");
}

#[test]
fn a_deferred_submission_returns_a_fence_that_eventually_signals() {
    let Some(mut ctx) = context() else { return };
    let mut tex = target(&mut ctx);
    let batch = scene();

    let fence = ctx
        .submit_batch_deferred(&mut tex, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("deferred submit");

    // Waiting is still available and is what a caller without fence export
    // falls back to. A timeout here would mean work was never submitted.
    assert!(
        fence.wait(FRAME_WAIT_TIMEOUT).expect("wait"),
        "the fence never signalled"
    );
    assert!(fence.is_signaled().expect("status"));

    ctx.retire_fence(fence);
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    ctx.destroy_texture(tex);
    assert_clean(&ctx);
}

#[test]
fn a_fence_exports_a_sync_file_before_the_work_completes() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sync.export_sync_file {
        eprintln!("skipping: this device cannot export a sync_file");
        return;
    }
    let mut tex = target(&mut ctx);
    let batch = scene();

    let fence = ctx
        .submit_batch_deferred(&mut tex, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("deferred submit");
    assert!(fence.is_exportable());

    // Exported without waiting first: that is the entire point. Waiting and
    // then exporting would produce a descriptor that is already signalled and
    // defeat the purpose of handing it to the kernel.
    match fence.export_sync_file() {
        Ok(fd) => {
            // A real, open descriptor. An invalid one fails only much later,
            // when the atomic commit rejects it.
            fd.try_clone()
                .expect("the exported sync_file is not an open descriptor");
        }
        Err(e) => {
            // The work may already have finished, in which case there is
            // nothing left to wait on and the kernel represents that as no
            // descriptor at all. That is a legitimate outcome for a trivial
            // scene on a fast device.
            let text = e.to_string();
            assert!(
                text.contains("already signalled"),
                "unexpected export failure: {text}"
            );
        }
    }

    assert!(fence.wait(FRAME_WAIT_TIMEOUT).expect("wait"));
    ctx.retire_fence(fence);
    ctx.destroy_texture(tex);
    assert_clean(&ctx);
}

#[test]
fn an_exported_sync_file_is_independent_of_the_fence_it_came_from() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sync.export_sync_file {
        return;
    }
    let mut tex = target(&mut ctx);
    let batch = scene();

    let fence = ctx
        .submit_batch_deferred(&mut tex, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("deferred submit");
    let exported = fence.export_sync_file().ok();

    assert!(fence.wait(FRAME_WAIT_TIMEOUT).expect("wait"));
    ctx.retire_fence(fence);

    // The descriptor outlives the fence, which is what handing it to another
    // process or to the kernel requires. Retiring the fence must not have
    // closed it.
    if let Some(fd) = exported {
        fd.try_clone()
            .expect("the sync_file was closed when the fence was retired");
    }

    ctx.destroy_texture(tex);
    assert_clean(&ctx);
}

#[test]
fn repeated_deferred_submissions_do_not_exhaust_resources() {
    let Some(mut ctx) = context() else { return };
    let mut tex = target(&mut ctx);
    let batch = scene();

    // Each submission holds a command buffer, a framebuffer, a view, and the
    // geometry buffers it is still reading. A frame loop does this every frame,
    // so anything not released on retire accumulates until something fails.
    for i in 0..64 {
        let fence = ctx
            .submit_batch_deferred(&mut tex, &batch, PassDescriptor::clear([0.0; 4]))
            .unwrap_or_else(|e| panic!("iteration {i}: {e}"));
        let _ = fence.export_sync_file();
        assert!(fence.wait(FRAME_WAIT_TIMEOUT).expect("wait"));
        ctx.retire_fence(fence);
    }

    let pixels = ctx.read_texture(&mut tex).expect("readback");
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    ctx.destroy_texture(tex);
    assert_clean(&ctx);
}

#[test]
fn deferred_and_waiting_submission_produce_the_same_pixels() {
    let Some(mut ctx) = context() else { return };
    let batch = scene();

    let mut deferred_target = target(&mut ctx);
    let fence = ctx
        .submit_batch_deferred(
            &mut deferred_target,
            &batch,
            PassDescriptor::clear([0.0, 0.0, 1.0, 1.0]),
        )
        .expect("deferred submit");
    assert!(fence.wait(FRAME_WAIT_TIMEOUT).expect("wait"));
    ctx.retire_fence(fence);
    let deferred = ctx.read_texture(&mut deferred_target).expect("readback");
    ctx.destroy_texture(deferred_target);

    let mut waiting_target = target(&mut ctx);
    ctx.submit_batch(
        &mut waiting_target,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 1.0, 1.0]),
    )
    .expect("submit");
    let waiting = ctx.read_texture(&mut waiting_target).expect("readback");
    ctx.destroy_texture(waiting_target);

    // Whether the caller waits is a scheduling decision, not a rendering one.
    assert_eq!(deferred, waiting, "deferring changed what was rendered");
    assert_clean(&ctx);
}

#[test]
fn a_deferred_multisampled_pass_is_refused_rather_than_leaking() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        return;
    }
    let mut tex = target(&mut ctx);
    let batch = scene();

    // The transient multisample buffer would have to outlive the submission
    // too. Refusing is better than submitting and freeing it underneath the
    // GPU, which corrupts intermittently rather than failing.
    let result = ctx.submit_batch_deferred(
        &mut tex,
        &batch,
        PassDescriptor::clear([0.0; 4]).with_samples(4),
    );
    assert!(result.is_err());

    ctx.destroy_texture(tex);
    assert_clean(&ctx);
}
