//! Rendering a scene to an image.
//!
//! Generic over the backend, so the same corpus runs against every one. This is
//! the offscreen executor; WSI and DRM executors render the same scenes through
//! their own presentation paths and compare with the same comparators.

use crate::image::Image;
use crate::scene::{Fill, Item, Scene};
use glam::{Affine2, Vec2};
use impeller_geometry::transform::viewport_projection;
use impeller_hal::{
    Batch, Extent2D, Hal, HalContext, Material, PassDescriptor, PixelFormat, Result, Stop,
    TextureDescriptor,
};
use impeller_renderer::{Paint, Renderer, TOLERANCE};

/// Resolve an item's fill against the transform it will be drawn under.
///
/// A gradient's endpoints have to travel through the same mapping the geometry
/// does, which needs both the item transform and the target size, so this is
/// the only place that has everything required.
fn material_for(item: &Item, transform: Affine2, target: Extent2D) -> Material {
    match &item.fill {
        Fill::Solid(color) => Material::solid(*color),
        Fill::LinearGradient { start, end, stops } => {
            let to_clip = viewport_projection(target.width, target.height) * transform;
            let start = to_clip.transform_point2(Vec2::from(*start));
            let end = to_clip.transform_point2(Vec2::from(*end));
            Material::LinearGradient {
                start: [start.x, start.y],
                end: [end.x, end.y],
                stops: stops
                    .iter()
                    .map(|stop| Stop::new(stop.color, stop.offset))
                    .collect(),
            }
        }
    }
}

/// Record a scene's items into a batch.
///
/// Shared rather than duplicated per caller. Turning scene data into paint
/// involves resolving gradient endpoints through two transforms, and a second
/// copy of that drifts out of step the moment the scene format grows — which is
/// what happened when gradients were added and a frame-loop test kept reading a
/// field that no longer existed.
pub fn record_scene(renderer: &mut Renderer, batch: &mut Batch, scene: &Scene) -> Result<()> {
    for item in &scene.items {
        let path = item.shape.to_path();
        let transform = item.transform.to_affine();
        let paint = Paint {
            material: material_for(item, transform, scene.size),
            blend: item.blend,
        };
        match &item.stroke {
            Some(spec) => {
                renderer.stroke_into(batch, &path, &spec.to_style(), transform, &paint)?
            }
            None => renderer.fill_into(batch, &path, transform, &paint)?,
        }
    }
    Ok(())
}

/// The pass a scene asks for.
pub fn pass_for(scene: &Scene) -> PassDescriptor {
    PassDescriptor::clear(scene.background).with_samples(scene.samples)
}

/// Render a scene offscreen and read it back.
pub fn render_scene<H: Hal>(ctx: &mut H::Context, scene: &Scene) -> Result<Image>
where
    H::Context: HalContext<Hal = H>,
{
    let mut renderer = Renderer::new();
    renderer.begin_frame(scene.size, TOLERANCE);
    let mut batch = Batch::new();
    record_scene(&mut renderer, &mut batch, scene)?;

    let mut target = ctx.create_texture(&TextureDescriptor::offscreen(
        scene.size,
        PixelFormat::Rgba8Unorm,
    ))?;

    let outcome = ctx
        .submit_batch(&mut target, &batch, pass_for(scene))
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
