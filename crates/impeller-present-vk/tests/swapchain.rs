//! The swapchain presentation path, against a surface with no window.
//!
//! `VK_EXT_headless_surface` is what makes this a real test rather than a
//! compile check. Everything the presentation target does — querying what the
//! surface allows, negotiating a format, acquiring, presenting, recreating —
//! runs exactly as it would against a window, and needs no display, no
//! compositor, and no window system library.
//!
//! What it does not exercise is what a window adds: an extent the surface
//! dictates, and a surface that goes out of date because something resized it.
//! Both are noted where the code handles them, and both are the parts a real
//! window would have to confirm.

use impeller_hal::{Batch, BlendMode, Extent2D, Material, PassDescriptor, PixelFormat, Vertex};
use impeller_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};
use impeller_present::PresentTarget;
use impeller_present_vk::{create_headless_surface, destroy_surface, PresentMode, SwapchainTarget};

const SIZE: Extent2D = Extent2D {
    width: 64,
    height: 64,
};
const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// A context and a headless surface, or nothing if either is unavailable.
fn setup() -> Option<(VulkanContext, ash::vk::SurfaceKHR)> {
    // Validation on, because this is where it earns its keep. A semaphore
    // signalled and never waited on, a wait on one nothing signals, an image
    // presented in the wrong layout: none of those show in the pixels, and two
    // of them are what the synchronization here exists to arrange.
    let ctx = match VulkanContext::with_config(ContextConfig {
        device: DevicePreference::Auto,
        validation: true,
    }) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no Vulkan device ({e})");
            return None;
        }
    };
    match create_headless_surface(&ctx) {
        Ok(surface) => Some((ctx, surface)),
        Err(e) => {
            eprintln!("skipping: no headless surface ({e})");
            None
        }
    }
}

/// Run `body` with a target, tearing everything down afterwards.
fn with_target(mode: PresentMode, body: impl FnOnce(&mut VulkanContext, &mut SwapchainTarget)) {
    let Some((mut ctx, surface)) = setup() else {
        return;
    };
    let mut target = match SwapchainTarget::new(&mut ctx, surface, SIZE, mode) {
        Ok(target) => target,
        Err(e) => {
            unsafe { destroy_surface(&ctx, surface) };
            panic!("swapchain: {e}");
        }
    };
    body(&mut ctx, &mut target);
    target.destroy(&mut ctx);

    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == impeller_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");
    // SAFETY: the swapchain built on it has just been destroyed.
    unsafe { destroy_surface(&ctx, surface) };
}

#[test]
fn a_swapchain_reports_a_usable_configuration() {
    with_target(PresentMode::Fifo, |_ctx, target| {
        // A headless surface defers its size, so this is the size asked for.
        // A swapchain silently one pixel across passes every other test here,
        // which is what this pins.
        assert_eq!(
            target.extent(),
            SIZE,
            "the swapchain ignored the size asked for"
        );
        // At least two, or the engine has nothing to display while the
        // application draws.
        assert!(
            target.image_count() >= 2,
            "a swapchain of {} image(s) cannot overlap drawing with display",
            target.image_count()
        );
        // Linear rather than sRGB: color is linear inside the renderer and the
        // attachment format applies the transfer function, so an sRGB surface
        // format would apply it to values that already carry it.
        assert!(
            matches!(
                target.format(),
                PixelFormat::Bgra8Unorm | PixelFormat::Rgba8Unorm
            ),
            "unexpected surface format {:?}",
            target.format()
        );
    });
}

#[test]
fn acquiring_and_presenting_advances_the_frame_count() {
    with_target(PresentMode::Fifo, |ctx, target| {
        assert_eq!(target.presented_frames(), 0);
        for expected in 1..=6u64 {
            target.acquire(ctx).expect("acquire");
            // Something has to be drawn, or the image is never transitioned out
            // of the undefined layout the engine handed it over in.
            let mut batch = Batch::new();
            let shade = expected as f32 / 6.0;
            batch
                .push(
                    &FULL,
                    &QUAD,
                    Material::solid([shade, 0.2, 1.0 - shade, 1.0]),
                    BlendMode::Src,
                )
                .expect("push");
            target
                .submit(ctx, &batch, PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]))
                .expect("submit");
            target.present(ctx).expect("present");
            assert_eq!(target.presented_frames(), expected);
        }
    });
}

#[test]
fn the_images_are_cycled_rather_than_reused_immediately() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // A swapchain that handed back the same image every time would let a
        // frame be drawn into an image the engine is still displaying. Reading
        // the pointer rather than an index because that is what the caller
        // actually renders into.
        let mut seen: Vec<*const u8> = Vec::new();
        for _ in 0..target.image_count() {
            let image = target.acquire(ctx).expect("acquire");
            seen.push(image as *const _ as *const u8);
            let mut batch = Batch::new();
            batch
                .push(&FULL, &QUAD, Material::solid([1.0; 4]), BlendMode::Src)
                .expect("push");
            target
                .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
                .expect("submit");
            target.present(ctx).expect("present");
        }
        seen.sort_unstable();
        let distinct = {
            seen.dedup();
            seen.len()
        };
        assert!(
            distinct > 1,
            "every acquisition returned the same image, so nothing is double buffered"
        );
    });
}

#[test]
fn presenting_without_acquiring_is_refused() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // An unbalanced pair means a frame index nobody holds, and presenting
        // one would display whatever that image last contained.
        assert!(target.present(ctx).is_err(), "present without acquire");
    });
}

#[test]
fn acquiring_twice_without_presenting_is_refused() {
    with_target(PresentMode::Fifo, |ctx, target| {
        let _ = target.acquire(ctx).expect("first acquire");
        assert!(
            target.acquire(ctx).is_err(),
            "two frames were acquired at once"
        );
    });
}

#[test]
fn reconfiguring_rebuilds_at_the_new_size() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // A headless surface defers its size to the swapchain, which is what
        // lets this ask for one. A windowed surface dictates instead, and the
        // request is ignored — the reason the extent is read back afterwards
        // rather than assumed.
        let before = target.extent();
        target
            .reconfigure(ctx, Extent2D::new(before.width * 2, before.height / 2))
            .expect("reconfigure");
        assert_ne!(target.extent(), before, "the swapchain did not change size");

        // And it still works afterwards: the old images are gone and the new
        // ones have to be usable, which is the part a rebuild gets wrong.
        let after = target.extent();
        let image = target.acquire(ctx).expect("acquire after reconfigure");
        assert_eq!(image.extent(), after);
        let _ = image;
        let mut batch = Batch::new();
        batch
            .push(
                &FULL,
                &QUAD,
                Material::solid([0.0, 1.0, 0.0, 1.0]),
                BlendMode::Src,
            )
            .expect("push");
        target
            .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");
        target.present(ctx).expect("present");
    });
}

#[test]
fn a_presented_frame_holds_what_was_drawn_into_it() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // Presenting is not observable without a display, so this checks the
        // half that is: the image handed out is a real render target, and what
        // is drawn into it is there afterwards. Without this the tests above
        // would pass on a target that handed back an image nothing reached.
        let _ = target.acquire(ctx).expect("acquire");
        let mut batch = Batch::new();
        batch
            .push(
                &FULL,
                &QUAD,
                Material::solid([0.25, 0.5, 0.75, 1.0]),
                BlendMode::Src,
            )
            .expect("push");
        target
            .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");

        let image = target.acquired_image().expect("acquired image");
        let extent = image.extent();
        let format = image.format();
        let pixels = ctx.read_texture(image).expect("readback");
        let middle = ((extent.height / 2 * extent.width + extent.width / 2) * 4) as usize;
        let got = [
            pixels[middle],
            pixels[middle + 1],
            pixels[middle + 2],
            pixels[middle + 3],
        ];
        // The surface may be BGRA, so the channels are compared as a set of
        // the two orderings rather than assumed.
        let rgba = [64u8, 128, 191, 255];
        let bgra = [191u8, 128, 64, 255];
        let near = |want: [u8; 4]| {
            got.iter()
                .zip(&want)
                .all(|(a, b)| (*a as i32 - *b as i32).abs() <= 1)
        };
        assert!(
            near(rgba) || near(bgra),
            "a presented image came back {got:?} in {format:?}"
        );

        target.present(ctx).expect("present");
    });
}

#[test]
fn a_mailbox_request_falls_back_rather_than_failing() {
    // Mailbox is optional, and a surface without it should still give a working
    // swapchain: it is a latency preference, not a correctness requirement.
    with_target(PresentMode::Mailbox, |ctx, target| {
        target.acquire(ctx).expect("acquire");
        let mut batch = Batch::new();
        batch
            .push(&FULL, &QUAD, Material::solid([1.0; 4]), BlendMode::Src)
            .expect("push");
        target
            .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");
        target.present(ctx).expect("present");
        assert_eq!(target.presented_frames(), 1);
    });
}

#[test]
fn nothing_blocks_until_the_frames_in_flight_are_exhausted() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // Acquiring and drawing are ordered by semaphores, so the first frames
        // do not block at all. A wait appears only once a slot has to be
        // reused, which is what bounds how far ahead of the display the
        // renderer runs — the mechanism working rather than a defect.
        assert_eq!(target.throttle_waits(), 0);
        for _ in 0..2 {
            target.acquire(ctx).expect("acquire");
            let mut batch = Batch::new();
            batch
                .push(&FULL, &QUAD, Material::solid([1.0; 4]), BlendMode::Src)
                .expect("push");
            target
                .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
                .expect("submit");
            target.present(ctx).expect("present");
        }
        assert_eq!(
            target.throttle_waits(),
            0,
            "a frame blocked before any slot needed reusing"
        );

        // The third frame reuses the first slot, and only then is there
        // anything to wait for.
        target.acquire(ctx).expect("acquire");
        let mut batch = Batch::new();
        batch
            .push(&FULL, &QUAD, Material::solid([1.0; 4]), BlendMode::Src)
            .expect("push");
        target
            .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");
        target.present(ctx).expect("present");
        assert_eq!(
            target.throttle_waits(),
            1,
            "reusing a frame slot did not wait for the frame that held it"
        );
    });
}

#[test]
fn a_swapchain_image_carries_texture_coordinates_like_any_other_target() {
    // Not about the swapchain as such: it checks that an image the presentation
    // engine allocated behaves as a render target in every respect, including
    // the parts a wrapper around somebody else's image is most likely to get
    // wrong. A target that only worked for solid fills would pass everything
    // above.
    with_target(PresentMode::Fifo, |ctx, target| {
        let _ = target.acquire(ctx).expect("acquire");
        let mut batch = Batch::new();
        let vertices: Vec<Vertex> = FULL.iter().map(|p| Vertex::at(*p)).collect();
        batch
            .push_mesh(
                &vertices,
                &QUAD,
                Material::LinearGradient {
                    start: [-1.0, 0.0],
                    end: [1.0, 0.0],
                    stops: vec![
                        impeller_hal::Stop::new([1.0, 0.0, 0.0, 1.0], 0.0),
                        impeller_hal::Stop::new([0.0, 0.0, 1.0, 1.0], 1.0),
                    ],
                },
                BlendMode::Src,
                None,
                impeller_hal::ClipState::UNCLIPPED,
            )
            .expect("push");
        target
            .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");

        let image = target.acquired_image().expect("acquired image");
        let extent = image.extent();
        let pixels = ctx.read_texture(image).expect("readback");
        let at = |x: u32| ((extent.height / 2 * extent.width + x) * 4) as usize;
        let left = pixels[at(2)];
        let right = pixels[at(extent.width - 3)];
        assert_ne!(
            left, right,
            "a gradient rendered flat into a swapchain image"
        );
        target.present(ctx).expect("present");
    });
}

#[test]
fn presenting_a_frame_drawn_the_generic_way_is_refused() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // Drawing through the backend-agnostic submission leaves acquisition's
        // semaphore signalled and never waited on, and presentation about to
        // wait on one nothing signalled. That hangs rather than looking wrong,
        // so it is refused instead.
        let image = target.acquire(ctx).expect("acquire");
        let mut batch = Batch::new();
        batch
            .push(&FULL, &QUAD, Material::solid([1.0; 4]), BlendMode::Src)
            .expect("push");
        ctx.submit_batch(image, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");
        assert!(
            target.present(ctx).is_err(),
            "a frame drawn without the target's own submit was presented"
        );

        // And the frame is still recoverable: drawing it properly presents.
        target
            .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");
        target.present(ctx).expect("present");
    });
}
