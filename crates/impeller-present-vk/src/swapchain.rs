//! Presenting through a `VkSwapchainKHR`.
//!
//! # Where the surface comes from
//!
//! A caller brings one. Creating windows is no more this project's business
//! than mode setting is: an application already has a window system connection
//! and a window, and asking it for the surface those imply is one call, where
//! owning that relationship would mean owning a windowing library.
//!
//! That boundary is also what makes this testable. `VK_EXT_headless_surface`
//! produces a surface with no window behind it, so the whole path — capability
//! query, format negotiation, present mode selection, acquire, present, and
//! recreation on resize — runs on a machine with no display, and every test
//! here does exactly that.
//!
//! # Synchronization
//!
//! Acquisition signals a semaphore, rendering waits on it, and presentation
//! waits on rendering. That is the shape the specification is written for, and
//! the rendering half of it is not reachable through the HAL as it stands:
//! `submit_batch` waits for completion on the CPU before returning, so by the
//! time a frame is presented the work is provably done and the present needs no
//! wait semaphore of its own.
//!
//! Acquisition is therefore gated with a fence and a CPU wait. Those waits are
//! counted rather than hidden, exactly as the scanout target counts its own:
//! a number that should be zero once the HAL grows submissions that take wait
//! and signal semaphores, and that is a frame of latency each until then.

use ash::vk;
use impeller_hal::{Error, Extent2D, PixelFormat, Result, FRAME_WAIT_TIMEOUT};
use impeller_hal_vulkan::{VulkanContext, VulkanHal, VulkanTexture};
use impeller_present::PresentTarget;

/// How the presentation engine should pace frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PresentMode {
    /// Queue frames and display them on refresh, never tearing and never
    /// dropping. The only mode a conformant implementation must offer, and the
    /// right default: it is the one that does not burn power drawing frames
    /// nobody sees.
    #[default]
    Fifo,
    /// Replace an undisplayed queued frame with a newer one.
    ///
    /// Lower latency at the cost of drawing frames that are discarded. Falls
    /// back to `Fifo` where the surface does not offer it, since it is
    /// optional and asking for it should not fail outright.
    Mailbox,
}

impl PresentMode {
    fn to_vk(self) -> vk::PresentModeKHR {
        match self {
            Self::Fifo => vk::PresentModeKHR::FIFO,
            Self::Mailbox => vk::PresentModeKHR::MAILBOX,
        }
    }
}

/// One acquired-but-not-presented frame.
struct Acquired {
    index: u32,
}

/// A swapchain, its images, and the frame in flight.
pub struct SwapchainTarget {
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    loader: ash::khr::swapchain::Device,
    surface_loader: ash::khr::surface::Instance,
    images: Vec<VulkanTexture>,
    acquired: Option<Acquired>,
    /// Signalled by acquisition, waited on before the image is handed out.
    acquire_fence: vk::Fence,
    extent: Extent2D,
    format: PixelFormat,
    vk_format: vk::Format,
    color_space: vk::ColorSpaceKHR,
    mode: PresentMode,
    presented: u64,
    cpu_waits: u64,
    /// Set when the engine reported the swapchain no longer matches its
    /// surface, so the next acquisition rebuilds before trying.
    stale: bool,
}

impl SwapchainTarget {
    /// Build a swapchain for a surface the caller created.
    ///
    /// The surface is borrowed, not adopted: it outlives this and is the
    /// caller's to destroy, because it was theirs to create and they may build
    /// another swapchain on it after this one is gone.
    ///
    /// `extent` is the size to ask for. A windowed surface dictates its own and
    /// ignores this; a surface that defers — a headless one does — takes it,
    /// clamped to what it allows. There is no useful default: deferring
    /// surfaces clamp an unstated size up from zero to the minimum, which is
    /// how a swapchain ends up one pixel across and every test of it still
    /// passes.
    pub fn new(
        ctx: &mut VulkanContext,
        surface: vk::SurfaceKHR,
        extent: Extent2D,
        mode: PresentMode,
    ) -> Result<Self> {
        let surface_loader = ash::khr::surface::Instance::new(ctx.raw_entry(), ctx.raw_instance());
        let loader = ash::khr::swapchain::Device::new(ctx.raw_instance(), ctx.raw_device());

        // A device that can render is not necessarily one that can present to
        // this surface: a headless card, or a second GPU with no connector.
        // Asking is the only way to know, and the answer is a capability rather
        // than something to infer from the device's identity.
        let supported = unsafe {
            surface_loader.get_physical_device_surface_support(
                ctx.raw_physical_device(),
                ctx.queue_family_index(),
                surface,
            )
        }
        .map_err(|e| backend_err("get_physical_device_surface_support", e))?;
        if !supported {
            return Err(Error::Unsupported(
                "this device's queue cannot present to this surface",
            ));
        }

        let fence_info = vk::FenceCreateInfo::default();
        let acquire_fence = unsafe { ctx.raw_device().create_fence(&fence_info, None) }
            .map_err(|e| backend_err("create_fence", e))?;

        let mut target = Self {
            surface,
            swapchain: vk::SwapchainKHR::null(),
            loader,
            surface_loader,
            images: Vec::new(),
            acquired: None,
            acquire_fence,
            extent: Extent2D::new(0, 0),
            format: PixelFormat::Bgra8Unorm,
            vk_format: vk::Format::B8G8R8A8_UNORM,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            mode,
            presented: 0,
            cpu_waits: 0,
            stale: false,
        };
        if let Err(e) = target.rebuild(ctx, Some(extent)) {
            unsafe { ctx.raw_device().destroy_fence(acquire_fence, None) };
            return Err(e);
        }
        Ok(target)
    }

    /// Frames handed to the presentation engine so far.
    pub fn presented_frames(&self) -> u64 {
        self.presented
    }

    /// How many times a frame blocked the CPU waiting for an acquisition.
    ///
    /// Counted rather than hidden because it is the cost of the HAL not yet
    /// taking wait semaphores, and a number nobody reports is a cost nobody
    /// removes.
    pub fn cpu_waits(&self) -> u64 {
        self.cpu_waits
    }

    /// How many images the engine gave us.
    pub fn image_count(&self) -> usize {
        self.images.len()
    }

    pub fn present_mode(&self) -> PresentMode {
        self.mode
    }

    /// Rebuild the swapchain, keeping the old one alive during the transition.
    ///
    /// `requested` is the extent to ask for, or `None` to take whatever the
    /// surface reports. A surface that dictates its size — every windowed one
    /// does — ignores the request, which is why the extent is read back
    /// afterwards rather than assumed.
    fn rebuild(&mut self, ctx: &mut VulkanContext, requested: Option<Extent2D>) -> Result<()> {
        let capabilities = unsafe {
            self.surface_loader
                .get_physical_device_surface_capabilities(ctx.raw_physical_device(), self.surface)
        }
        .map_err(|e| backend_err("get_physical_device_surface_capabilities", e))?;

        let formats = unsafe {
            self.surface_loader
                .get_physical_device_surface_formats(ctx.raw_physical_device(), self.surface)
        }
        .map_err(|e| backend_err("get_physical_device_surface_formats", e))?;
        let (vk_format, color_space, format) = choose_format(&formats)?;

        let modes = unsafe {
            self.surface_loader
                .get_physical_device_surface_present_modes(ctx.raw_physical_device(), self.surface)
        }
        .map_err(|e| backend_err("get_physical_device_surface_present_modes", e))?;
        // FIFO is required of every implementation, so falling back to it
        // always succeeds; asking for mailbox where it is absent degrades
        // rather than failing, since it is a latency preference and not a
        // correctness requirement.
        let wanted = self.mode.to_vk();
        let present_mode = if modes.contains(&wanted) {
            wanted
        } else {
            vk::PresentModeKHR::FIFO
        };

        let extent = resolve_extent(&capabilities, requested.unwrap_or(self.extent));
        if extent.width == 0 || extent.height == 0 {
            // A minimized window reports a zero extent, and a swapchain of that
            // size is invalid. Reporting rather than creating one lets a frame
            // loop skip the frame, which is what it would have to do anyway.
            return Err(Error::Unsupported(
                "the surface has no area; nothing can be presented into it",
            ));
        }

        // One more than the minimum, so the engine always has an image to
        // display while the application draws into another. Clamped to the
        // maximum, which is zero when there is none.
        let mut count = capabilities.min_image_count + 1;
        if capabilities.max_image_count > 0 {
            count = count.min(capabilities.max_image_count);
        }

        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface)
            .min_image_count(count)
            .image_format(vk_format)
            .image_color_space(color_space)
            .image_extent(vk::Extent2D {
                width: extent.width,
                height: extent.height,
            })
            .image_array_layers(1)
            // Transfer destination as well as attachment, because reading a
            // presented frame back is how a test knows what was shown, and a
            // clear of an empty batch goes through a transfer.
            .image_usage(
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::TRANSFER_DST,
            )
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(choose_composite_alpha(&capabilities))
            .present_mode(present_mode)
            // Pixels the window system obscures may be undefined, which is what
            // lets the engine skip rendering them. Nothing here reads a
            // presented image except through an explicit readback in a test,
            // and that reads the image it just drew rather than a displayed one.
            .clipped(true)
            .old_swapchain(self.swapchain);

        let swapchain = unsafe { self.loader.create_swapchain(&info, None) }
            .map_err(|e| backend_err("create_swapchain", e))?;

        // The old swapchain is retired by creating the new one but still has to
        // be destroyed, and only after nothing is using its images.
        self.release_images(ctx);
        if self.swapchain != vk::SwapchainKHR::null() {
            unsafe { self.loader.destroy_swapchain(self.swapchain, None) };
        }
        self.swapchain = swapchain;

        let images = unsafe { self.loader.get_swapchain_images(swapchain) }
            .map_err(|e| backend_err("get_swapchain_images", e))?;
        self.images = images
            .into_iter()
            // Undefined is right: an image the engine has just handed over
            // holds nothing anybody may read, and every pass either clears it
            // or transitions from whatever it left behind.
            .map(|image| {
                VulkanTexture::wrap_image(image, extent, format, vk::ImageLayout::UNDEFINED)
            })
            .collect();

        self.extent = extent;
        self.format = format;
        self.vk_format = vk_format;
        self.color_space = color_space;
        self.stale = false;
        Ok(())
    }

    fn release_images(&mut self, ctx: &mut VulkanContext) {
        for texture in self.images.drain(..) {
            // Releases the wrapper and nothing else: the engine owns these.
            ctx.destroy_texture(texture);
        }
        self.acquired = None;
    }

    /// Wait for the acquisition fence and reset it for the next frame.
    fn wait_for_acquisition(&mut self, ctx: &VulkanContext) -> Result<()> {
        let device = ctx.raw_device();
        self.cpu_waits += 1;
        // SAFETY: the fence was submitted to by the acquisition above and is
        // not being waited on elsewhere.
        unsafe {
            device
                .wait_for_fences(
                    &[self.acquire_fence],
                    true,
                    FRAME_WAIT_TIMEOUT.as_nanos() as u64,
                )
                .map_err(|e| backend_err("wait_for_fences", e))?;
            device
                .reset_fences(&[self.acquire_fence])
                .map_err(|e| backend_err("reset_fences", e))?;
        }
        Ok(())
    }
}

impl PresentTarget<VulkanHal> for SwapchainTarget {
    fn extent(&self) -> Extent2D {
        self.extent
    }

    fn format(&self) -> PixelFormat {
        self.format
    }

    fn acquire(&mut self, ctx: &mut VulkanContext) -> Result<&mut VulkanTexture> {
        if self.acquired.is_some() {
            return Err(Error::Unsupported(
                "acquire without a matching present; one frame is already in flight",
            ));
        }
        if self.stale {
            self.rebuild(ctx, None)?;
        }

        // Two attempts at most: the first may find the swapchain out of date,
        // and the rebuild that follows produces one that matches the surface,
        // so a second failure is a real error rather than a race.
        for attempt in 0..2 {
            let result = unsafe {
                self.loader.acquire_next_image(
                    self.swapchain,
                    FRAME_WAIT_TIMEOUT.as_nanos() as u64,
                    vk::Semaphore::null(),
                    self.acquire_fence,
                )
            };
            match result {
                // Suboptimal still produced an image, and refusing it would
                // drop a frame that is merely imperfect. The rebuild happens
                // on the next acquisition instead.
                Ok((index, suboptimal)) => {
                    self.stale = suboptimal;
                    self.wait_for_acquisition(ctx)?;
                    self.acquired = Some(Acquired { index });
                    return Ok(&mut self.images[index as usize]);
                }
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) if attempt == 0 => {
                    self.rebuild(ctx, None)?;
                }
                Err(e) => return Err(backend_err("acquire_next_image", e)),
            }
        }
        Err(Error::Unsupported(
            "the swapchain went out of date twice in a row",
        ))
    }

    fn present(&mut self, ctx: &mut VulkanContext) -> Result<()> {
        let Some(acquired) = self.acquired.take() else {
            return Err(Error::Unsupported("present without a matching acquire"));
        };

        // Presenting reads the image, so it has to be in the layout the engine
        // expects rather than whatever the last pass left it in.
        ctx.transition_for_present(&self.images[acquired.index as usize])?;

        let swapchains = [self.swapchain];
        let indices = [acquired.index];
        let info = vk::PresentInfoKHR::default()
            .swapchains(&swapchains)
            .image_indices(&indices);

        // No wait semaphore: submission waits for completion on the CPU before
        // returning, so the work is provably finished by the time this runs.
        // That is the stall `cpu_waits` counts, and the place a signal
        // semaphore belongs once the HAL can take one.
        let result = unsafe { self.loader.queue_present(ctx.raw_queue(), &info) };
        match result {
            Ok(suboptimal) => {
                self.stale |= suboptimal;
                self.presented += 1;
                Ok(())
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                // The frame is lost, which is not an error: the surface changed
                // under it. The next acquisition rebuilds.
                self.stale = true;
                Ok(())
            }
            Err(e) => Err(backend_err("queue_present", e)),
        }
    }

    fn reconfigure(&mut self, ctx: &mut VulkanContext, extent: Extent2D) -> Result<()> {
        self.rebuild(ctx, Some(extent))
    }

    fn destroy(mut self, ctx: &mut VulkanContext) {
        self.release_images(ctx);
        // SAFETY: every submission was waited on before the call that made it
        // returned, so nothing is reading these.
        unsafe {
            if self.swapchain != vk::SwapchainKHR::null() {
                self.loader.destroy_swapchain(self.swapchain, None);
            }
            ctx.raw_device().destroy_fence(self.acquire_fence, None);
        }
        // The surface is the caller's, and stays.
    }
}

/// Pick a surface format, preferring one the renderer already speaks.
///
/// A surface offering `UNDEFINED` alone means it has no preference, which is
/// what a headless surface reports on some drivers; there the choice is ours
/// outright.
fn choose_format(
    formats: &[vk::SurfaceFormatKHR],
) -> Result<(vk::Format, vk::ColorSpaceKHR, PixelFormat)> {
    if formats.is_empty() {
        return Err(Error::Unsupported("the surface offers no formats"));
    }
    if formats.len() == 1 && formats[0].format == vk::Format::UNDEFINED {
        return Ok((
            vk::Format::B8G8R8A8_UNORM,
            vk::ColorSpaceKHR::SRGB_NONLINEAR,
            PixelFormat::Bgra8Unorm,
        ));
    }

    // In preference order, and all of them non-sRGB: color is linear inside
    // this renderer and the conversion is the attachment format's job, so
    // picking an sRGB surface format would apply the transfer function to
    // values that already carry it.
    let wanted = [
        (vk::Format::B8G8R8A8_UNORM, PixelFormat::Bgra8Unorm),
        (vk::Format::R8G8B8A8_UNORM, PixelFormat::Rgba8Unorm),
    ];
    for (vk_format, format) in wanted {
        if let Some(found) = formats
            .iter()
            .find(|f| f.format == vk_format && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
        {
            return Ok((found.format, found.color_space, format));
        }
    }
    Err(Error::Unsupported(
        "the surface offers no format this renderer can target",
    ))
}

/// The extent to build for, given what the surface allows.
///
/// A surface reporting a current extent of `u32::MAX` is saying the swapchain
/// decides — that is how a surface without a fixed size answers — so the
/// request is used, clamped to the allowed range. Anything else dictates, and
/// the request is ignored: asking a window for a size it did not offer produces
/// an invalid swapchain rather than a resized window.
fn resolve_extent(capabilities: &vk::SurfaceCapabilitiesKHR, requested: Extent2D) -> Extent2D {
    if capabilities.current_extent.width != u32::MAX {
        return Extent2D::new(
            capabilities.current_extent.width,
            capabilities.current_extent.height,
        );
    }
    Extent2D::new(
        requested.width.clamp(
            capabilities.min_image_extent.width,
            capabilities.max_image_extent.width,
        ),
        requested.height.clamp(
            capabilities.min_image_extent.height,
            capabilities.max_image_extent.height,
        ),
    )
}

/// How the surface's alpha meets the desktop behind it.
///
/// Opaque where it is offered, which is what a window without a transparency
/// request wants, and the only mode every implementation is likely to have.
/// Inherit is the fallback because it defers to whatever the platform already
/// decided, which is more likely right than picking a blend mode blind.
fn choose_composite_alpha(capabilities: &vk::SurfaceCapabilitiesKHR) -> vk::CompositeAlphaFlagsKHR {
    for candidate in [
        vk::CompositeAlphaFlagsKHR::OPAQUE,
        vk::CompositeAlphaFlagsKHR::INHERIT,
        vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
    ] {
        if capabilities.supported_composite_alpha.contains(candidate) {
            return candidate;
        }
    }
    vk::CompositeAlphaFlagsKHR::OPAQUE
}

fn backend_err(what: &str, e: vk::Result) -> Error {
    Error::Backend {
        backend: "vulkan",
        detail: format!("{what}: {e:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(current: (u32, u32), min: (u32, u32), max: (u32, u32)) -> vk::SurfaceCapabilitiesKHR {
        vk::SurfaceCapabilitiesKHR {
            current_extent: vk::Extent2D {
                width: current.0,
                height: current.1,
            },
            min_image_extent: vk::Extent2D {
                width: min.0,
                height: min.1,
            },
            max_image_extent: vk::Extent2D {
                width: max.0,
                height: max.1,
            },
            ..Default::default()
        }
    }

    #[test]
    fn a_surface_with_a_fixed_size_dictates_it() {
        // Every windowed surface does. Building at a size it did not offer
        // produces an invalid swapchain rather than a resized window, so the
        // request is ignored rather than clamped toward.
        let capabilities = caps((800, 600), (1, 1), (4096, 4096));
        assert_eq!(
            resolve_extent(&capabilities, Extent2D::new(1920, 1080)),
            Extent2D::new(800, 600)
        );
    }

    #[test]
    fn a_surface_that_defers_takes_the_request_within_its_range() {
        let capabilities = caps((u32::MAX, u32::MAX), (16, 16), (1024, 1024));
        assert_eq!(
            resolve_extent(&capabilities, Extent2D::new(640, 480)),
            Extent2D::new(640, 480)
        );
        // And a request outside the range is clamped rather than refused: the
        // surface said it decides, and this is it deciding.
        assert_eq!(
            resolve_extent(&capabilities, Extent2D::new(4096, 8)),
            Extent2D::new(1024, 16)
        );
    }

    #[test]
    fn a_surface_with_no_preference_gets_the_renderer_s_own_choice() {
        let formats = [vk::SurfaceFormatKHR {
            format: vk::Format::UNDEFINED,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
        }];
        let (vk_format, _, format) = choose_format(&formats).expect("choose");
        assert_eq!(vk_format, vk::Format::B8G8R8A8_UNORM);
        assert_eq!(format, PixelFormat::Bgra8Unorm);
    }

    #[test]
    fn an_srgb_surface_format_is_not_chosen_over_a_linear_one() {
        // Color is linear inside this renderer and the attachment format
        // applies the transfer function, so picking an sRGB surface format
        // would apply it to values that already carry it.
        let formats = [
            vk::SurfaceFormatKHR {
                format: vk::Format::B8G8R8A8_SRGB,
                color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            },
            vk::SurfaceFormatKHR {
                format: vk::Format::B8G8R8A8_UNORM,
                color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            },
        ];
        let (vk_format, _, _) = choose_format(&formats).expect("choose");
        assert_eq!(vk_format, vk::Format::B8G8R8A8_UNORM);
    }

    #[test]
    fn a_surface_offering_nothing_usable_is_refused() {
        let formats = [vk::SurfaceFormatKHR {
            format: vk::Format::R5G6B5_UNORM_PACK16,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
        }];
        assert!(choose_format(&formats).is_err());
        assert!(choose_format(&[]).is_err());
    }

    #[test]
    fn composite_alpha_prefers_opaque_and_never_returns_nothing() {
        let with = |supported| vk::SurfaceCapabilitiesKHR {
            supported_composite_alpha: supported,
            ..Default::default()
        };
        assert_eq!(
            choose_composite_alpha(&with(
                vk::CompositeAlphaFlagsKHR::OPAQUE | vk::CompositeAlphaFlagsKHR::INHERIT
            )),
            vk::CompositeAlphaFlagsKHR::OPAQUE
        );
        assert_eq!(
            choose_composite_alpha(&with(vk::CompositeAlphaFlagsKHR::INHERIT)),
            vk::CompositeAlphaFlagsKHR::INHERIT
        );
    }
}
