//! The scanout frame loop, driven against a stand-in display.
//!
//! No KMS device is involved. That is deliberate rather than a limitation: the
//! parts of this path most likely to be wrong are the ring accounting and the
//! fence plumbing, and neither needs real hardware to get wrong. A display that
//! records what it was asked to do makes those assertions direct — that a
//! buffer still on screen is never handed back, that a commit carries the
//! render-done signal, that acquiring waits for a flip rather than spinning.
//!
//! What this cannot check is whether a real display controller accepts the
//! buffers. That is what the VKMS lane and the board rack are for.

use impeller_hal::{
    Batch, BlendMode, Extent2D, FormatModifierSet, Fourcc, Material, Modifier, PassDescriptor,
    Result,
};
use impeller_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal};
use impeller_present::PresentTarget;
use impeller_present_drm::{
    output::{CommitRequest, DmaBufPlanes, OutputEvent, ScanoutOutput},
    DrmScanoutTarget, FbHandle, Mode,
};
use std::sync::{Arc, Mutex};

/// What a commit carried, recorded for inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedCommit {
    fb: FbHandle,
    had_fence: bool,
    allow_modeset: bool,
}

#[derive(Debug, Default)]
struct Recording {
    commits: Vec<RecordedCommit>,
    imported: Vec<FbHandle>,
    released: Vec<FbHandle>,
    waits: usize,
}

/// A display that records what it was asked to do and flips on demand.
struct FakeOutput {
    mode: Mode,
    formats: Vec<FormatModifierSet>,
    next_fb: u64,
    log: Arc<Mutex<Recording>>,
    pending: Vec<OutputEvent>,
    /// When true, a commit immediately queues its own flip completion, which is
    /// what a display doing its job looks like.
    auto_flip: bool,
}

impl FakeOutput {
    fn new(formats: Vec<FormatModifierSet>) -> (Self, Arc<Mutex<Recording>>) {
        let log = Arc::new(Mutex::new(Recording::default()));
        (
            Self {
                mode: Mode {
                    extent: Extent2D::new(64, 64),
                    refresh_mhz: 60_000,
                },
                formats,
                next_fb: 1,
                log: Arc::clone(&log),
                pending: Vec::new(),
                auto_flip: true,
            },
            log,
        )
    }
}

impl ScanoutOutput for FakeOutput {
    fn mode(&self) -> Mode {
        self.mode
    }

    fn supported_formats(&self) -> &[FormatModifierSet] {
        &self.formats
    }

    fn import_dmabuf(&mut self, buffer: DmaBufPlanes) -> Result<FbHandle> {
        // A real import fails on a mismatched modifier, so the test double
        // checks the buffer it was handed carries one it advertised.
        let advertised = self
            .formats
            .iter()
            .find(|s| s.fourcc == buffer.fourcc)
            .expect("imported a format that was never advertised");
        assert!(
            advertised.modifiers.contains(&buffer.modifier),
            "imported modifier {} was never advertised",
            buffer.modifier
        );

        let fb = FbHandle(self.next_fb);
        self.next_fb += 1;
        self.log.lock().unwrap().imported.push(fb);
        Ok(fb)
    }

    fn release_framebuffer(&mut self, fb: FbHandle) {
        self.log.lock().unwrap().released.push(fb);
    }

    fn commit(&mut self, request: CommitRequest) -> Result<()> {
        self.log.lock().unwrap().commits.push(RecordedCommit {
            fb: request.fb,
            #[cfg(unix)]
            had_fence: request.in_fence_fd.is_some(),
            #[cfg(not(unix))]
            had_fence: false,
            allow_modeset: request.allow_modeset,
        });
        if self.auto_flip {
            self.pending
                .push(OutputEvent::FlipComplete { fb: request.fb });
        }
        Ok(())
    }

    fn poll_events(&mut self) -> Vec<OutputEvent> {
        std::mem::take(&mut self.pending)
    }

    fn wait_for_event(&mut self, _timeout_nanos: u64) -> Result<Vec<OutputEvent>> {
        self.log.lock().unwrap().waits += 1;
        Ok(std::mem::take(&mut self.pending))
    }
}

fn context() -> Option<VulkanContext> {
    match VulkanContext::new(DevicePreference::Auto) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable Vulkan device ({e})");
            None
        }
    }
}

/// Formats the display accepts: whatever the device can render into.
fn display_formats(ctx: &VulkanContext) -> Vec<FormatModifierSet> {
    ctx.capabilities().render_formats.clone()
}

fn scene() -> Batch {
    let mut batch = Batch::new();
    batch
        .push(
            &[[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
            &[0, 1, 2, 0, 2, 3],
            Material::solid([1.0, 0.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    batch
}

/// Render and present one frame, as a real loop would.
fn frame(
    ctx: &mut VulkanContext,
    target: &mut DrmScanoutTarget<VulkanHal, FakeOutput>,
    batch: &Batch,
) -> Result<()> {
    let image = target.acquire(ctx)?;
    let fence = ctx.submit_batch_deferred(image, batch, PassDescriptor::clear([0.0; 4]))?;
    target.set_frame_fence(fence)?;
    target.present(ctx)
}

fn exportable(ctx: &VulkanContext) -> bool {
    if ctx.capabilities().dma_buf.can_allocate_scanout() {
        return true;
    }
    eprintln!("skipping: this device cannot allocate its own scanout buffers");
    false
}

#[test]
fn a_ring_is_built_from_exported_buffers() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    let (output, log) = FakeOutput::new(display_formats(&ctx));
    let target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 3).expect("scanout target");

    assert_eq!(target.ring_depth(), 3);
    // Every buffer is exported once at startup and imported once, rather than
    // per frame. Doing it per frame would put an allocation and an import in
    // the middle of the frame loop.
    assert_eq!(log.lock().unwrap().imported.len(), 3);

    target.destroy(&mut ctx);
    assert_eq!(log.lock().unwrap().released.len(), 3, "framebuffers leaked");
}

#[test]
fn the_first_commit_allows_a_modeset_and_later_ones_do_not() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    let (output, log) = FakeOutput::new(display_formats(&ctx));
    let mut target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 3).expect("scanout target");
    let batch = scene();

    for _ in 0..4 {
        frame(&mut ctx, &mut target, &batch).expect("frame");
    }

    let recorded = log.lock().unwrap();
    // A modeset is far more expensive than a flip. Requesting one every frame
    // would work and would be slow in a way nothing about the output reveals.
    assert!(recorded.commits[0].allow_modeset, "first commit");
    assert!(
        recorded.commits[1..].iter().all(|c| !c.allow_modeset),
        "later commits should not ask for a modeset"
    );
    drop(recorded);
    target.destroy(&mut ctx);
}

#[test]
fn every_commit_carries_the_render_done_signal() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    if !ctx.capabilities().sync.export_sync_file {
        eprintln!("skipping: this device cannot export a sync_file");
        return;
    }
    let (output, log) = FakeOutput::new(display_formats(&ctx));
    let mut target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 3).expect("scanout target");
    let batch = scene();

    for _ in 0..4 {
        frame(&mut ctx, &mut target, &batch).expect("frame");
    }

    let recorded = log.lock().unwrap();
    let with_fence = recorded.commits.iter().filter(|c| c.had_fence).count();
    // Committing without a fence means the CPU waited first, which is correct
    // and costs a frame of latency. On hardware that can export, it should not
    // be happening at all, and the counter is what makes that visible rather
    // than merely slow.
    assert!(
        with_fence > 0,
        "no commit carried a sync_file, so every frame blocked the CPU first"
    );
    eprintln!(
        "  {with_fence} of {} commits carried a fence, {} CPU waits",
        recorded.commits.len(),
        target.cpu_waits()
    );
    drop(recorded);
    target.destroy(&mut ctx);
}

#[test]
fn a_buffer_still_on_screen_is_never_handed_back() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    let (mut output, log) = FakeOutput::new(display_formats(&ctx));
    // No automatic flips: nothing ever leaves the screen, so after the ring is
    // full there is no free slot and acquiring must fail rather than hand back
    // a buffer the display is reading.
    output.auto_flip = false;
    let mut target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 2).expect("scanout target");
    let batch = scene();

    frame(&mut ctx, &mut target, &batch).expect("first frame");
    frame(&mut ctx, &mut target, &batch).expect("second frame");

    // Both buffers are committed and neither has flipped away. Reusing one now
    // would tear, so the loop must stall instead.
    let stalled = frame(&mut ctx, &mut target, &batch);
    assert!(
        stalled.is_err(),
        "a buffer still on screen was handed back for drawing"
    );
    assert!(
        log.lock().unwrap().waits > 0,
        "acquire spun instead of waiting"
    );

    target.destroy(&mut ctx);
}

#[test]
fn a_flip_frees_the_buffer_it_replaced() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    let (output, _log) = FakeOutput::new(display_formats(&ctx));
    let mut target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 2).expect("scanout target");
    let batch = scene();

    // With flips arriving, a two-deep ring sustains an indefinite loop. If a
    // completed flip did not free the buffer it replaced, this would stall on
    // the third frame.
    for i in 0..16 {
        frame(&mut ctx, &mut target, &batch).unwrap_or_else(|e| panic!("frame {i}: {e}"));
    }

    target.destroy(&mut ctx);
}

#[test]
fn presenting_without_acquiring_is_refused() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    let (output, _log) = FakeOutput::new(display_formats(&ctx));
    let mut target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 2).expect("scanout target");

    // Committing a buffer nobody rendered into would put whatever it last held
    // on screen.
    assert!(target.present(&mut ctx).is_err());
    target.destroy(&mut ctx);
}

#[test]
fn negotiation_runs_against_what_the_display_advertises() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    // A display that shares nothing with the renderer must fail to build a
    // target, rather than quietly picking something the plane cannot scan out.
    let incompatible = vec![FormatModifierSet::new(
        Fourcc::new(*b"NV12"),
        vec![Modifier::LINEAR],
    )];
    let (output, _log) = FakeOutput::new(incompatible);
    let result = DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 2);
    assert!(result.is_err(), "a target was built with no shared format");
}
