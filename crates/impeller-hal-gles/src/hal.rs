//! The backend's implementation of the rendering HAL.

use crate::context::GlesContext;
use crate::render::GlesTexture;
use impeller_hal::{
    Batch, Capabilities, Extent2D, Hal, HalContext, HalTexture, PassDescriptor, PixelFormat,
    Result, TextureDescriptor,
};

/// Marker naming the GLES backend's family of types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlesHal;

impl Hal for GlesHal {
    type Context = GlesContext;
    type Texture = GlesTexture;
    /// A fence type exists to satisfy the trait, but nothing constructs one:
    /// deferred submission is not implemented here, so the default returns
    /// unsupported and this is never produced. Naming an uninhabited type is
    /// more honest than naming one that could be built but never signals.
    type Fence = std::convert::Infallible;

    const NAME: &'static str = "gles";
}

impl HalTexture for GlesTexture {
    fn extent(&self) -> Extent2D {
        GlesTexture::extent(self)
    }

    fn format(&self) -> PixelFormat {
        GlesTexture::format(self)
    }
}

impl HalContext for GlesContext {
    type Hal = GlesHal;

    fn capabilities(&self) -> &Capabilities {
        GlesContext::capabilities(self)
    }

    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<GlesTexture> {
        GlesContext::create_texture(self, desc)
    }

    fn destroy_texture(&mut self, texture: GlesTexture) {
        GlesContext::destroy_texture(self, texture)
    }

    fn register_program(&mut self, program: &impeller_hal::RuntimeProgram) -> Result<u32> {
        GlesContext::register_program(self, program)
    }

    fn submit_batch(
        &mut self,
        target: &mut GlesTexture,
        batch: &Batch,
        pass: PassDescriptor,
    ) -> Result<()> {
        GlesContext::submit_batch(self, target, batch, pass)
    }

    fn submit_batch_textured(
        &mut self,
        target: &mut GlesTexture,
        batch: &Batch,
        pass: PassDescriptor,
        textures: &[&GlesTexture],
    ) -> Result<()> {
        GlesContext::submit_batch_textured(self, target, batch, pass, textures)
    }

    fn read_texture(&mut self, texture: &mut GlesTexture) -> Result<Vec<u8>> {
        GlesContext::read_texture(self, texture)
    }

    fn write_texture(&mut self, texture: &mut GlesTexture, pixels: &[u8]) -> Result<()> {
        GlesContext::write_texture(self, texture, pixels)
    }
}
