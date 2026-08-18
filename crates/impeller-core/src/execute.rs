//! Rendering a recording, pass by pass.
//!
//! Here rather than in the crate that wraps a backend, because a recording is
//! defined here and executing one is nothing more than reading it: a layer is a
//! target allocated at that pass's size, rendered into, and sampled by a later
//! pass. Nothing about that is backend-specific, so it is written once and
//! generic over the HAL. It had been private to the public crate, which left
//! the test harness with the choice of duplicating it or staying at the batch
//! level where layers cannot be expressed.

use crate::canvas::{Recording, TextureSource};
use impeller_hal::{Error, Hal, HalContext, PixelFormat, Result, TextureDescriptor};

/// Render every pass of a recording, layers first, root into the surface.
///
/// `images` are the caller's own textures, addressed by the slot a paint named.
/// A pass's slot table mixes them with the layers it samples, and resolving it
/// per pass is what lets the two number independently.
///
/// Layer targets are allocated per call and released before returning. Reusing
/// them across frames is worth doing and is a pool's job; doing it here would
/// mean a cache whose invalidation rule has to answer what happens when the
/// surface is resized.
pub fn execute<H: Hal>(
    ctx: &mut H::Context,
    surface: &mut H::Texture,
    recording: &Recording,
    images: &[&H::Texture],
) -> Result<()>
where
    H::Context: HalContext<Hal = H>,
{
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
                let mut target = ctx.create_texture(&TextureDescriptor::offscreen(
                    pass.extent,
                    PixelFormat::Rgba8Unorm,
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

/// Render a recording into a texture this function allocates, and read it back.
///
/// The offscreen path, which is what a comparison run wants: no surface, no
/// presentation, just the pixels. The target is released whichever way the
/// render went, so a failing recording does not leak a texture into the rest of
/// a run.
pub fn render_offscreen<H: Hal>(
    ctx: &mut H::Context,
    recording: &Recording,
    images: &[&H::Texture],
) -> Result<Vec<u8>>
where
    H::Context: HalContext<Hal = H>,
{
    let mut target = ctx.create_texture(&TextureDescriptor::offscreen(
        recording.extent,
        PixelFormat::Rgba8Unorm,
    ))?;
    let outcome = execute::<H>(ctx, &mut target, recording, images)
        .and_then(|()| ctx.read_texture(&mut target));
    ctx.destroy_texture(target);
    outcome
}
