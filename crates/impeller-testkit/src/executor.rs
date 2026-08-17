//! Rendering a scene to an image.
//!
//! Generic over the backend, so the same corpus runs against every one. This is
//! the offscreen executor; WSI and DRM executors render the same scenes through
//! their own presentation paths and compare with the same comparators.

use crate::image::Image;
use crate::scene::Scene;
use impeller_hal::{
    Batch, Hal, HalContext, PassDescriptor, PixelFormat, Result, TextureDescriptor,
};
use impeller_renderer::{Paint, Renderer, TOLERANCE};

/// Render a scene offscreen and read it back.
pub fn render_scene<H: Hal>(ctx: &mut H::Context, scene: &Scene) -> Result<Image>
where
    H::Context: HalContext<Hal = H>,
{
    let mut renderer = Renderer::new();
    renderer.begin_frame(scene.size, TOLERANCE);

    let mut batch = Batch::new();
    for item in &scene.items {
        let path = item.shape.to_path();
        let transform = item.transform.to_affine();
        let paint = Paint {
            color: item.color,
            blend: item.blend,
        };
        match &item.stroke {
            Some(spec) => {
                renderer.stroke_into(&mut batch, &path, &spec.to_style(), transform, paint)?
            }
            None => renderer.fill_into(&mut batch, &path, transform, paint)?,
        }
    }

    let mut target = ctx.create_texture(&TextureDescriptor::offscreen(
        scene.size,
        PixelFormat::Rgba8Unorm,
    ))?;

    let outcome = ctx
        .submit_batch(
            &mut target,
            &batch,
            PassDescriptor::clear(scene.background).with_samples(scene.samples),
        )
        .and_then(|()| ctx.read_texture(&mut target));

    // The target is released whichever way the submission went, so a failing
    // scene does not leak a texture into the rest of the run.
    ctx.destroy_texture(target);

    Ok(Image::new(scene.size.width, scene.size.height, outcome?))
}

/// Render every scene in a corpus.
///
/// A scene that fails is reported with its name rather than aborting the run,
/// so one broken capability does not hide the state of everything else.
pub fn render_corpus<H: Hal>(
    ctx: &mut H::Context,
    scenes: &[Scene],
) -> Vec<(&'static str, Result<Image>)>
where
    H::Context: HalContext<Hal = H>,
{
    scenes
        .iter()
        .map(|scene| (scene.name, render_scene::<H>(ctx, scene)))
        .collect()
}
