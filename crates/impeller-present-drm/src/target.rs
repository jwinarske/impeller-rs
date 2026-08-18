//! Direct scanout: rendering straight to a display with no compositor.

use crate::output::{CommitRequest, DmaBufPlanes, FbHandle, OutputEvent, ScanoutOutput};
use impeller_hal::{
    Error, Extent2D, Hal, HalContext, HalFence, PixelFormat, Result, FRAME_WAIT_TIMEOUT,
};
use impeller_present::{negotiate, PresentTarget, PREFERRED_FORMATS};

/// How many buffers the ring holds by default.
///
/// Two is enough to avoid tearing but leaves the GPU idle while a frame is on
/// screen. Three lets rendering start on the next frame immediately, at the
/// cost of one more buffer and one frame of latency.
pub const DEFAULT_RING_DEPTH: usize = 3;

/// One buffer in the ring, and what is currently true of it.
struct Slot<H: Hal> {
    texture: H::Texture,
    fb: FbHandle,
    /// Set while this buffer has been committed but not yet replaced on screen.
    ///
    /// The ring must never hand back a buffer in this state: the display is
    /// still reading it, and drawing into it produces tearing or worse.
    on_screen: bool,
    /// The submission that last rendered into this buffer, if it has not been
    /// retired yet.
    fence: Option<H::Fence>,
}

/// A presentation target that scans out directly to a display.
///
/// The frame loop is the same as every other target — acquire, render, present
/// — but the pacing is different in kind. A window's swapchain decides when a
/// slot is free; here the display does, by telling us a flip completed. Waiting
/// on that is what keeps the loop at the panel's refresh rate without a timer.
pub struct DrmScanoutTarget<H: Hal, O: ScanoutOutput> {
    output: O,
    slots: Vec<Slot<H>>,
    /// Which slot the caller is currently drawing into.
    acquired: Option<usize>,
    format: PixelFormat,
    extent: Extent2D,
    /// Set once the first commit has established the mode.
    mode_set: bool,
    /// Whether a commit is waiting for its flip.
    ///
    /// A display controller takes one at a time, which is a smaller number
    /// than the ring depth and a different thing from it: the ring bounds
    /// buffers in flight so the renderer can work ahead, and this bounds
    /// commits so the kernel does not refuse one.
    flip_pending: bool,
    /// Commits made without a sync_file attached, because the device could not
    /// export one.
    cpu_waits: u64,
}

impl<H: Hal, O: ScanoutOutput> DrmScanoutTarget<H, O>
where
    H::Context: HalContext<Hal = H>,
{
    /// Build a scanout target, negotiating a shared format with the display.
    pub fn new(ctx: &mut H::Context, output: O, depth: usize) -> Result<Self> {
        let capabilities = ctx.capabilities();
        if !capabilities.dma_buf.can_allocate_scanout() {
            // The other path is GBM-allocated buffers imported into the
            // renderer. Reporting which one applies belongs to capabilities, so
            // this refuses rather than silently doing something else.
            return Err(Error::Unsupported(
                "this device cannot allocate its own scanout buffers; use the GBM path",
            ));
        }
        let render_formats = capabilities.render_formats.clone();
        let chosen = negotiate(
            &render_formats,
            output.supported_formats(),
            PREFERRED_FORMATS,
        )?;
        let format = pixel_format_for(chosen.fourcc)?;
        let extent = output.mode().extent;

        let candidates = [chosen.modifier];
        let mut target = Self {
            output,
            slots: Vec::with_capacity(depth),
            acquired: None,
            format,
            extent,
            mode_set: false,
            flip_pending: false,
            cpu_waits: 0,
        };
        target.build_ring(ctx, depth.max(2), &candidates)?;
        Ok(target)
    }

    /// Buffers in the ring.
    pub fn ring_depth(&self) -> usize {
        self.slots.len()
    }

    /// How many commits went out without a sync_file attached.
    ///
    /// Every one of these is a frame where the CPU waited before committing.
    /// Correct, but a frame of latency each time, and on hardware that should
    /// support fence export it means something is misconfigured rather than
    /// missing.
    pub fn cpu_waits(&self) -> u64 {
        self.cpu_waits
    }

    pub fn output(&self) -> &O {
        &self.output
    }

    fn build_ring(
        &mut self,
        ctx: &mut H::Context,
        depth: usize,
        modifiers: &[impeller_hal::Modifier],
    ) -> Result<()> {
        for _ in 0..depth {
            let texture = ctx.create_exportable_texture(self.extent, self.format, modifiers)?;
            let exported = ctx.export_texture(&texture)?;
            let fb = self.output.import_dmabuf(DmaBufPlanes {
                #[cfg(unix)]
                planes: exported.planes,
                fourcc: exported.fourcc,
                modifier: exported.modifier,
                extent: self.extent,
            })?;
            self.slots.push(Slot {
                texture,
                fb,
                on_screen: false,
                fence: None,
            });
        }
        Ok(())
    }

    /// Absorb whatever the display has told us since last time.
    fn drain_events(&mut self, events: Vec<OutputEvent>) -> bool {
        let mut reconfigured = false;
        for event in events {
            match event {
                OutputEvent::FlipComplete { fb } => {
                    // The buffer that was on screen before this one is now
                    // free. Marking only the one that just landed would leak
                    // the ring: nothing would ever be released.
                    for slot in &mut self.slots {
                        if slot.fb != fb {
                            slot.on_screen = false;
                        }
                    }
                    self.flip_pending = false;
                }
                OutputEvent::Reconfigured => reconfigured = true,
            }
        }
        reconfigured
    }

    /// Block until the flip already committed has landed.
    ///
    /// Returns immediately where none is outstanding, which is every frame on a
    /// target whose display keeps up.
    fn await_pending_flip(&mut self) -> Result<()> {
        if !self.flip_pending {
            return Ok(());
        }
        let budget = self.output.mode().frame_nanos().max(1);
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_nanos(budget.saturating_mul(8).max(1_000_000));
        while self.flip_pending {
            let events = self.output.wait_for_event(budget)?;
            let reconfigured = self.drain_events(events);
            if reconfigured {
                return Err(Error::Unsupported("output reconfigured; rebuild the ring"));
            }

            if self.flip_pending && std::time::Instant::now() >= deadline {
                // The display has stopped. Blocking forever would hide that,
                // and committing anyway would be refused.
                return Err(Error::Timeout);
            }
        }
        Ok(())
    }

    /// Index of a slot that is neither on screen nor still being rendered into.
    fn free_slot(&mut self, ctx: &mut H::Context) -> Option<usize> {
        for i in 0..self.slots.len() {
            if self.slots[i].on_screen {
                continue;
            }
            // A slot whose previous submission has not completed is not free
            // either: drawing into it would race the GPU still reading it.
            if let Some(fence) = &self.slots[i].fence {
                match fence.is_signaled() {
                    Ok(true) => {}
                    _ => continue,
                }
            }
            if let Some(fence) = self.slots[i].fence.take() {
                ctx.retire_fence(fence);
            }
            return Some(i);
        }
        None
    }

    /// A slot that has left the screen but whose rendering is still in flight.
    ///
    /// The distinction matters for what to wait on. A slot held by the display
    /// frees when a flip completes; a slot held by the GPU frees when its fence
    /// signals. Waiting for a display event in the second case waits for
    /// something that is not coming.
    fn slot_awaiting_gpu(&self) -> Option<usize> {
        (0..self.slots.len()).find(|i| !self.slots[*i].on_screen && self.slots[*i].fence.is_some())
    }
}

impl<H: Hal, O: ScanoutOutput> PresentTarget<H> for DrmScanoutTarget<H, O>
where
    H::Context: HalContext<Hal = H>,
{
    fn extent(&self) -> Extent2D {
        self.extent
    }

    fn format(&self) -> PixelFormat {
        self.format
    }

    fn acquire(&mut self, ctx: &mut H::Context) -> Result<&mut H::Texture> {
        if let Some(index) = self.acquired {
            return Ok(&mut self.slots[index].texture);
        }

        let budget = self.output.mode().frame_nanos().max(1);
        // Several frame times of patience, measured on the clock rather than in
        // iterations, then give up. A slot that never frees means the display
        // or the GPU has stopped, and blocking forever would hide that.
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_nanos(budget.saturating_mul(8).max(1_000_000));

        loop {
            let events = self.output.poll_events();
            if self.drain_events(events) {
                return Err(Error::Unsupported("output reconfigured; rebuild the ring"));
            }
            if let Some(index) = self.free_slot(ctx) {
                self.acquired = Some(index);
                return Ok(&mut self.slots[index].texture);
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::Timeout);
            }

            // What to wait on depends on what is holding the slot. A slot the
            // display still owns frees on a flip; one the GPU still owns frees
            // when its fence signals, and waiting for a display event there
            // waits for something that is not coming.
            if let Some(index) = self.slot_awaiting_gpu() {
                if let Some(fence) = &self.slots[index].fence {
                    fence.wait(FRAME_WAIT_TIMEOUT)?;
                    continue;
                }
            }
            let events = self.output.wait_for_event(budget)?;
            if self.drain_events(events) {
                return Err(Error::Unsupported("output reconfigured; rebuild the ring"));
            }
        }
    }

    fn present(&mut self, ctx: &mut H::Context) -> Result<()> {
        let Some(index) = self.acquired.take() else {
            return Err(Error::Unsupported("present without a matching acquire"));
        };

        // Waited for before the fence is taken out of its slot, not after. A
        // failure here leaves the slot holding its fence, so teardown can still
        // wait on it; taking it first and failing afterwards drops it
        // un-retired, and the buffer it was still rendering into gets freed
        // while the GPU is reading it.
        if let Err(e) = self.await_pending_flip() {
            self.acquired = Some(index);
            return Err(e);
        }

        // The fence for this frame's rendering was stored by the caller through
        // `submit`; without one there is nothing to attach and nothing to gate
        // reuse on.
        let fence = self.slots[index].fence.take();
        // Exported where the device can, and waited on where it cannot. The
        // error is handled after the match rather than inside it, so the fence
        // can be put back: a failure that dropped it would leave teardown with
        // nothing to wait on and a buffer freed while the GPU reads it.
        #[cfg(unix)]
        let mut in_fence_fd = None;
        #[cfg(unix)]
        let wait_failed = match &fence {
            Some(f) => match f.export_sync_file() {
                Ok(fd) => {
                    in_fence_fd = Some(fd);
                    None
                }
                Err(_) => {
                    // Either the device cannot export, or the work already
                    // finished so there is nothing left to wait on. Both mean
                    // committing without a fence, and the first is worth
                    // counting because it costs a frame of latency every time.
                    self.cpu_waits += 1;
                    f.wait(FRAME_WAIT_TIMEOUT).err()
                }
            },
            None => None,
        };
        #[cfg(unix)]
        if let Some(e) = wait_failed {
            self.slots[index].fence = fence;
            self.acquired = Some(index);
            return Err(e);
        }

        let request = CommitRequest {
            fb: self.slots[index].fb,
            #[cfg(unix)]
            in_fence_fd,
            allow_modeset: !self.mode_set,
        };
        self.output.commit(request)?;
        self.flip_pending = true;
        self.mode_set = true;
        self.slots[index].on_screen = true;
        self.slots[index].fence = fence;
        let _ = ctx;
        Ok(())
    }

    fn reconfigure(&mut self, ctx: &mut H::Context, extent: Extent2D) -> Result<()> {
        if extent == self.extent && !self.slots.is_empty() {
            return Ok(());
        }
        let depth = self.slots.len().max(2);
        let modifiers = vec![impeller_hal::Modifier::LINEAR];
        // Everything is rebuilt: a new mode means new buffer dimensions, and a
        // framebuffer built for the old one cannot be scanned out.
        self.release(ctx);
        self.extent = extent;
        self.mode_set = false;
        self.build_ring(ctx, depth, &modifiers)
    }

    fn destroy(mut self, ctx: &mut H::Context) {
        self.release(ctx);
    }
}

impl<H: Hal, O: ScanoutOutput> DrmScanoutTarget<H, O>
where
    H::Context: HalContext<Hal = H>,
{
    fn release(&mut self, ctx: &mut H::Context) {
        for mut slot in self.slots.drain(..) {
            // Waiting first: the display may still be reading a buffer, and
            // releasing a framebuffer out from under a live scanout is the
            // failure that shows up as a screen full of garbage.
            if let Some(fence) = slot.fence.take() {
                let _ = fence.wait(FRAME_WAIT_TIMEOUT);
                ctx.retire_fence(fence);
            }
            self.output.release_framebuffer(slot.fb);
            ctx.destroy_texture(slot.texture);
        }
        self.acquired = None;
    }

    /// Record the fence for the frame currently being rendered.
    ///
    /// Separate from `present` because the caller submits the work and so is
    /// the one holding the fence; the target needs it both to attach to the
    /// commit and to decide when the slot may be reused.
    pub fn set_frame_fence(&mut self, fence: H::Fence) -> Result<()> {
        let Some(index) = self.acquired else {
            return Err(Error::Unsupported("no frame is currently acquired"));
        };
        self.slots[index].fence = Some(fence);
        Ok(())
    }
}

/// Map a scanout format code onto the renderer's format.
fn pixel_format_for(fourcc: impeller_hal::Fourcc) -> Result<PixelFormat> {
    use impeller_hal::Fourcc;
    Ok(match fourcc {
        f if f == Fourcc::ARGB8888 || f == Fourcc::XRGB8888 => PixelFormat::Bgra8Unorm,
        f if f == Fourcc::ABGR8888 || f == Fourcc::XBGR8888 => PixelFormat::Rgba8Unorm,
        f if f == Fourcc::XRGB2101010 || f == Fourcc::ARGB2101010 => PixelFormat::Rgb10A2Unorm,
        _ => {
            return Err(Error::Unsupported(
                "negotiation chose a format the renderer cannot target",
            ))
        }
    })
}
