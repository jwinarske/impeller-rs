//! Rendering a recording, pass by pass.
//!
//! Here rather than in the crate that wraps a backend, because a recording is
//! defined here and executing one is nothing more than reading it: a layer is a
//! target allocated at that pass's size, rendered into, and sampled by a later
//! pass. Nothing about that is backend-specific, so it is written once and
//! generic over the HAL. It had been private to the public crate, which left
//! the test harness with the choice of duplicating it or staying at the batch
//! level where layers cannot be expressed.

use crate::canvas::{Pass, Recording, TextureSource};
use impeller_hal::{Error, Hal, HalContext, PixelFormat, Result, TextureDescriptor};

/// Resolve one pass's slot table against the caller's images and the layers
/// rendered so far.
///
/// A slot means something different in each pass, since a layer occupies one
/// alongside the caller's images and the two number independently. Resolving
/// per pass is what lets that work.
pub fn resolve_sources<'a, H: Hal>(
    pass: &Pass,
    images: &[&'a H::Texture],
    layers: &'a [H::Texture],
) -> Result<Vec<&'a H::Texture>> {
    let mut table = Vec::with_capacity(pass.sources.len());
    for source in &pass.sources {
        match source {
            TextureSource::Image(slot) => {
                table.push(*images.get(*slot as usize).ok_or(Error::Unsupported(
                    "a paint samples an image the caller did not supply",
                ))?);
            }
            // Earlier by construction, so this is always already rendered. A
            // recording that named a later one would be malformed rather than
            // merely out of order.
            TextureSource::Layer(pass_index) => {
                table.push(layers.get(*pass_index).ok_or(Error::Unsupported(
                    "a layer is composited before it is rendered",
                ))?);
            }
        }
    }
    Ok(table)
}

/// Render every pass except the root, returning the layer targets in order.
///
/// Split out from [`execute`] so that a caller who has to submit the root pass
/// itself can still get the layers rendered. That is not a hypothetical: the
/// root is the pass that lands in a presentable image, and presenting one means
/// submitting it gated on the acquire and present semaphores — which is a
/// different call from the one that renders offscreen and waits. Without this
/// split, a frame with layers could be rendered offscreen and never presented.
///
/// The returned textures are the caller's to destroy, and must outlive the
/// submission that samples them.
pub fn execute_layers<H: Hal>(
    ctx: &mut H::Context,
    recording: &Recording,
    images: &[&H::Texture],
) -> Result<Vec<H::Texture>>
where
    H::Context: HalContext<Hal = H>,
{
    // Rendered in order, so a layer is always finished before the pass that
    // samples it -- which is the order a recording stores them in, since a
    // layer is filed when it is restored and cannot be composited before that.
    let mut layers: Vec<H::Texture> = Vec::new();
    for pass in &recording.passes[..recording.passes.len() - 1] {
        let outcome = resolve_sources::<H>(pass, images, &layers).and_then(|table| {
            let mut target = ctx.create_texture(&TextureDescriptor::offscreen(
                pass.extent,
                PixelFormat::Rgba8Unorm,
            ))?;
            let result =
                ctx.submit_batch_textured(&mut target, &pass.batch, pass.descriptor, &table);
            Ok((target, result))
        });
        match outcome {
            Ok((target, result)) => {
                // The target joins the list whether or not the draw succeeded,
                // so a failure releases it with the rest rather than leaking.
                layers.push(target);
                if let Err(e) = result {
                    destroy_all::<H>(ctx, layers);
                    return Err(e);
                }
            }
            Err(e) => {
                destroy_all::<H>(ctx, layers);
                return Err(e);
            }
        }
    }
    Ok(layers)
}

fn destroy_all<H: Hal>(ctx: &mut H::Context, layers: Vec<H::Texture>)
where
    H::Context: HalContext<Hal = H>,
{
    for layer in layers {
        ctx.destroy_texture(layer);
    }
}

/// Render every pass of a recording, layers first, root into the surface.
///
/// `images` are the caller's own textures, addressed by the slot a paint named.
///
/// Layer targets are allocated per call and released before returning. Pooling
/// them was assumed to be worth doing and is not: creating and destroying a
/// 512x512 target measures 0.6 microseconds against 53 for the submission that
/// draws into it, so a pool would save under two percent of what a pass costs
/// and would owe an invalidation rule in exchange.
pub fn execute<H: Hal>(
    ctx: &mut H::Context,
    surface: &mut H::Texture,
    recording: &Recording,
    images: &[&H::Texture],
) -> Result<()>
where
    H::Context: HalContext<Hal = H>,
{
    let layers = execute_layers::<H>(ctx, recording, images)?;
    let root = recording.root();
    let outcome = resolve_sources::<H>(root, images, &layers)
        .and_then(|table| ctx.submit_batch_textured(surface, &root.batch, root.descriptor, &table));
    destroy_all::<H>(ctx, layers);
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

/// Render a recording's layers, then submit its root without waiting.
///
/// The shape a display path needs. Everything but the root is a prerequisite of
/// it rather than part of the frame's pacing, so those go through the waiting
/// submission and are ordered before the root by having finished; only the root
/// produces a fence, because the fence is what a page flip is gated on and the
/// root is the pass that fills the image being flipped.
///
/// Both halves of the return travel together until the fence retires: the layer
/// targets are what the submission is still sampling. Handing them back rather
/// than hiding them is deliberate — a caller that owns the fence is the only
/// one that knows when it retires.
pub fn execute_deferred<H: Hal>(
    ctx: &mut H::Context,
    surface: &mut H::Texture,
    recording: &Recording,
    images: &[&H::Texture],
) -> Result<(H::Fence, Vec<H::Texture>)>
where
    H::Context: HalContext<Hal = H>,
{
    let layers = execute_layers::<H>(ctx, recording, images)?;
    let root = recording.root();
    let outcome = resolve_sources::<H>(root, images, &layers).and_then(|table| {
        ctx.submit_batch_deferred_textured(surface, &root.batch, root.descriptor, &table)
    });
    match outcome {
        Ok(fence) => Ok((fence, layers)),
        Err(e) => {
            // No fence means nothing is reading them, so this is the moment.
            destroy_all::<H>(ctx, layers);
            Err(e)
        }
    }
}
