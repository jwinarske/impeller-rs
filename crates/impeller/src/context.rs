//! A rendering context, over whichever backend is available.

use crate::backend::{no_backend, Backend, BackendPreference};
use impeller_core::Recording;
use impeller_hal::{Capabilities, Error, Extent2D, HalContext, PixelFormat, Result};

/// A device to render with.
///
/// Which backend is behind it is decided at creation and reported by
/// [`Context::backend`]. Callers branch on [`Context::capabilities`] rather than
/// on which backend it turned out to be: a device without fence export is the
/// same problem whether it is Vulkan or GLES.
pub enum Context {
    #[cfg(feature = "vulkan")]
    Vulkan(impeller_hal_vulkan::VulkanContext),
    #[cfg(feature = "gles")]
    Gles(impeller_hal_gles::GlesContext),
}

impl Context {
    /// Create a context, choosing a backend.
    pub fn new(preference: BackendPreference) -> Result<Self> {
        let mut attempts = Vec::new();

        #[cfg(feature = "vulkan")]
        if matches!(
            preference,
            BackendPreference::Auto | BackendPreference::Vulkan
        ) {
            match impeller_hal_vulkan::VulkanContext::new(
                impeller_hal_vulkan::DevicePreference::Auto,
            ) {
                Ok(ctx) => return Ok(Self::Vulkan(ctx)),
                Err(e) => attempts.push((Backend::Vulkan, e)),
            }
        }

        #[cfg(feature = "gles")]
        if matches!(
            preference,
            BackendPreference::Auto | BackendPreference::Gles
        ) {
            match impeller_hal_gles::GlesContext::new(impeller_hal_gles::DisplayTarget::Surfaceless)
            {
                Ok(ctx) => return Ok(Self::Gles(ctx)),
                Err(e) => attempts.push((Backend::Gles, e)),
            }
        }

        let _ = preference;
        Err(no_backend(attempts))
    }

    pub fn backend(&self) -> Backend {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(_) => Backend::Vulkan,
            #[cfg(feature = "gles")]
            Self::Gles(_) => Backend::Gles,
        }
    }

    pub fn capabilities(&self) -> &Capabilities {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(ctx) => HalContext::capabilities(ctx),
            #[cfg(feature = "gles")]
            Self::Gles(ctx) => HalContext::capabilities(ctx),
        }
    }

    /// Create a surface to draw into.
    pub fn create_surface(&mut self, extent: Extent2D, format: PixelFormat) -> Result<Surface> {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(ctx) => Ok(Surface::Vulkan(HalContext::create_texture(
                ctx,
                &impeller_hal::TextureDescriptor::offscreen(extent, format),
            )?)),
            #[cfg(feature = "gles")]
            Self::Gles(ctx) => Ok(Surface::Gles(HalContext::create_texture(
                ctx,
                &impeller_hal::TextureDescriptor::offscreen(extent, format),
            )?)),
        }
    }

    /// Draw a recording into a surface.
    pub fn draw(&mut self, surface: &mut Surface, recording: &Recording) -> Result<()> {
        self.draw_with_images(surface, recording, &[])
    }

    /// Draw a recording whose paints sample images.
    ///
    /// `images` is the table a [`Shader::Image`] slot indexes, in slot order.
    /// A recording carries slots rather than images because it is produced
    /// without touching a device; supplying the table here is what resolves
    /// them.
    ///
    /// The surface is borrowed mutably and the images immutably, so a recording
    /// cannot sample the surface it draws into. That restriction is real, and
    /// having the compiler state it beats discovering it as a picture that
    /// differs by driver.
    ///
    /// [`Shader::Image`]: impeller_core::Shader::Image
    pub fn draw_with_images(
        &mut self,
        surface: &mut Surface,
        recording: &Recording,
        images: &[&Image],
    ) -> Result<()> {
        match (self, surface) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Surface::Vulkan(texture)) => {
                let images = vulkan_textures(images)?;
                execute::<impeller_hal_vulkan::VulkanHal>(ctx, texture, recording, &images)
            }
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Surface::Gles(texture)) => {
                let images = gles_textures(images)?;
                execute::<impeller_hal_gles::GlesHal>(ctx, texture, recording, &images)
            }
            // A surface belongs to the context that made it; pairing one with
            // another context would use a handle the device never allocated.
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported(
                "this surface was created by a different context",
            )),
        }
    }

    /// Allocate an image this context can sample.
    pub fn create_image(&mut self, extent: Extent2D, format: PixelFormat) -> Result<Image> {
        let descriptor = impeller_hal::TextureDescriptor::offscreen(extent, format);
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(ctx) => HalContext::create_texture(ctx, &descriptor).map(Image::Vulkan),
            #[cfg(feature = "gles")]
            Self::Gles(ctx) => HalContext::create_texture(ctx, &descriptor).map(Image::Gles),
        }
    }

    /// Fill an image from host memory, tightly packed and top row first.
    ///
    /// Decoding is out of scope for this crate; bring `image` or another
    /// decoder and hand the pixels here.
    pub fn write_image(&mut self, image: &mut Image, pixels: &[u8]) -> Result<()> {
        match (self, image) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Image::Vulkan(texture)) => {
                HalContext::write_texture(ctx, texture, pixels)
            }
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Image::Gles(texture)) => {
                HalContext::write_texture(ctx, texture, pixels)
            }
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported(
                "this image was created by a different context",
            )),
        }
    }

    /// Release an image.
    ///
    /// Explicit for the same reason a surface is: the memory belongs to the
    /// context, which an image cannot reach from its own `Drop`.
    pub fn destroy_image(&mut self, image: Image) {
        match (self, image) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Image::Vulkan(texture)) => {
                HalContext::destroy_texture(ctx, texture)
            }
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Image::Gles(texture)) => HalContext::destroy_texture(ctx, texture),
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    /// Read a surface back as tightly packed RGBA8.
    pub fn read(&mut self, surface: &mut Surface) -> Result<Vec<u8>> {
        match (self, surface) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Surface::Vulkan(texture)) => HalContext::read_texture(ctx, texture),
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Surface::Gles(texture)) => HalContext::read_texture(ctx, texture),
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported(
                "this surface was created by a different context",
            )),
        }
    }

    /// Release a surface.
    ///
    /// Explicit because the memory belongs to the context, which a surface
    /// cannot reach from its own `Drop`.
    pub fn destroy_surface(&mut self, surface: Surface) {
        match (self, surface) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Surface::Vulkan(texture)) => {
                HalContext::destroy_texture(ctx, texture)
            }
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Surface::Gles(texture)) => HalContext::destroy_texture(ctx, texture),
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
}

/// Something to draw into.
/// A texture a paint can sample.
///
/// Distinct from a [`Surface`] in the type system even though both are textures
/// underneath, because the two are used in opposite directions and mixing them
/// up is exactly the mistake worth making impossible: a surface is drawn into,
/// an image is read from.
pub enum Image {
    #[cfg(feature = "vulkan")]
    Vulkan(impeller_hal_vulkan::VulkanTexture),
    #[cfg(feature = "gles")]
    Gles(impeller_hal_gles::GlesTexture),
}

impl Image {
    pub fn extent(&self) -> Extent2D {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(texture) => texture.extent(),
            #[cfg(feature = "gles")]
            Self::Gles(texture) => texture.extent(),
        }
    }
}

/// Render every pass of a recording, layers first, root into the surface.
///
/// Generic over the HAL rather than written once per backend, because nothing
/// here is backend-specific: a layer is a target allocated at the recording's
/// size, rendered into, and sampled by a later pass. Writing it twice would be
/// two chances to get the ordering wrong.
///
/// Layer targets are allocated per call and released before returning. Reusing
/// them across frames is worth doing and is a pool's job; doing it here would
/// mean a cache whose invalidation rule has to answer what happens when the
/// surface is resized.
fn execute<H: impeller_hal::Hal>(
    ctx: &mut H::Context,
    surface: &mut H::Texture,
    recording: &Recording,
    images: &[&H::Texture],
) -> Result<()>
where
    H::Context: HalContext<Hal = H>,
{
    use impeller_core::TextureSource;

    // Rendered in order, so a layer is always finished before the pass that
    // samples it -- which is the order a recording stores them in, since a
    // layer is filed when it is restored and cannot be composited before that.
    let mut layers: Vec<H::Texture> = Vec::new();
    let outcome = (|| -> Result<()> {
        for (index, pass) in recording.passes.iter().enumerate() {
            let is_root = index + 1 == recording.passes.len();

            // Resolved per pass: a slot means something different in each,
            // since a layer occupies one and the caller's images occupy others.
            let mut table: Vec<&H::Texture> = Vec::with_capacity(pass.sources.len());
            for source in &pass.sources {
                match source {
                    TextureSource::Image(slot) => {
                        let texture = images.get(*slot as usize).ok_or(Error::Unsupported(
                            "a paint samples an image the caller did not supply",
                        ))?;
                        table.push(texture);
                    }
                    // Earlier by construction, so this is always already
                    // rendered. A recording that named a later one would be
                    // malformed rather than merely out of order.
                    TextureSource::Layer(pass_index) => {
                        let texture = layers.get(*pass_index).ok_or(Error::Unsupported(
                            "a layer is composited before it is rendered",
                        ))?;
                        table.push(texture);
                    }
                }
            }

            if is_root {
                ctx.submit_batch_textured(surface, &pass.batch, pass.descriptor, &table)?;
            } else {
                let mut target =
                    ctx.create_texture(&impeller_hal::TextureDescriptor::offscreen(
                        pass.extent,
                        impeller_hal::PixelFormat::Rgba8Unorm,
                    ))?;
                let result =
                    ctx.submit_batch_textured(&mut target, &pass.batch, pass.descriptor, &table);
                layers.push(target);
                result?;
            }
        }
        Ok(())
    })();

    for layer in layers {
        ctx.destroy_texture(layer);
    }
    outcome
}

/// Unwrap a table of images to one backend's textures.
///
/// An image from another backend is an error rather than a skip: the slots are
/// positional, so dropping one would silently shift every slot after it.
#[cfg(feature = "vulkan")]
fn vulkan_textures<'a>(
    images: &[&'a Image],
) -> Result<Vec<&'a impeller_hal_vulkan::VulkanTexture>> {
    images
        .iter()
        .map(|image| match image {
            Image::Vulkan(texture) => Ok(texture),
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported(
                "this image was created by a different context",
            )),
        })
        .collect()
}

#[cfg(feature = "gles")]
fn gles_textures<'a>(images: &[&'a Image]) -> Result<Vec<&'a impeller_hal_gles::GlesTexture>> {
    images
        .iter()
        .map(|image| match image {
            Image::Gles(texture) => Ok(texture),
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported(
                "this image was created by a different context",
            )),
        })
        .collect()
}

pub enum Surface {
    #[cfg(feature = "vulkan")]
    Vulkan(impeller_hal_vulkan::VulkanTexture),
    #[cfg(feature = "gles")]
    Gles(impeller_hal_gles::GlesTexture),
}

impl Surface {
    pub fn extent(&self) -> Extent2D {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(texture) => texture.extent(),
            #[cfg(feature = "gles")]
            Self::Gles(texture) => texture.extent(),
        }
    }

    pub fn format(&self) -> PixelFormat {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(texture) => texture.format(),
            #[cfg(feature = "gles")]
            Self::Gles(texture) => texture.format(),
        }
    }
}
