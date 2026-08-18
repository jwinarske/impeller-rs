//! Rendering a scene to an image.
//!
//! Generic over the backend, so the same corpus runs against every one. This is
//! the offscreen executor; WSI and DRM executors render the same scenes through
//! their own presentation paths and compare with the same comparators.

use crate::image::Image;
use crate::scene::{Fill, Item, Scene};
use crate::shape::Shape;
use glam::{Affine2, Mat2, Vec2};
use impeller_geometry::transform::{transformed_bounds, viewport_projection};
use impeller_hal::{
    Batch, BlendMode, ClipState, Extent2D, Hal, HalContext, Material, PassDescriptor, PixelFormat,
    Result, Scissor, Stop, TextureDescriptor,
};
use impeller_renderer::{Paint, Renderer, TOLERANCE};

/// Resolve an item's fill against the transform it will be drawn under.
///
/// A gradient's endpoints have to travel through the same mapping the geometry
/// does, which needs both the item transform and the target size, so this is
/// the only place that has everything required.
fn material_for(item: &Item, transform: Affine2, target: Extent2D) -> Material {
    let to_clip = viewport_projection(target.width, target.height) * transform;
    let convert = |stops: &[crate::scene::Stop]| -> Vec<Stop> {
        stops
            .iter()
            .map(|stop| Stop::new(stop.color, stop.offset))
            .collect()
    };

    match &item.fill {
        Fill::Solid(color) => Material::solid(*color),
        Fill::LinearGradient { start, end, stops } => {
            let axis = Vec2::from(*end) - Vec2::from(*start);
            let start = to_clip.transform_point2(Vec2::from(*start));
            Material::LinearGradient {
                start: [start.x, start.y],
                axis: [axis.x, axis.y],
                // The axis is in the scene's own space, so the shader is told
                // how to get back there from clip space. Taking the difference
                // in clip space instead lets the target's aspect ratio into the
                // gradient's direction, which is what it used to do.
                to_local: invert_or_identity(to_clip.matrix2),
                stops: convert(stops),
            }
        }
        Fill::RadialGradient {
            center,
            radius,
            stops,
        } => {
            let center_clip = to_clip.transform_point2(Vec2::from(*center));
            // The radius folds into the mapping, so the shader measures against
            // unit distance and never sees a radius.
            let scaled = to_clip.matrix2 * Mat2::from_diagonal(Vec2::splat(*radius));
            Material::RadialGradient {
                center: [center_clip.x, center_clip.y],
                to_local: invert_or_identity(scaled),
                stops: convert(stops),
            }
        }
        Fill::SweepGradient {
            center,
            start_angle,
            end_angle,
            stops,
        } => {
            let center_clip = to_clip.transform_point2(Vec2::from(*center));
            Material::SweepGradient {
                center: [center_clip.x, center_clip.y],
                to_local: invert_or_identity(to_clip.matrix2),
                start_angle: *start_angle,
                end_angle: *end_angle,
                stops: convert(stops),
            }
        }
    }
}

/// Invert a mapping, falling back to the identity where it cannot be inverted.
///
/// A degenerate transform collapses the shape to nothing, so the gradient it
/// would have carried is not observable; returning the identity keeps a
/// non-finite matrix out of the shader, where it would spread NaN across the
/// whole draw.
fn invert_or_identity(matrix: Mat2) -> [f32; 4] {
    let determinant = matrix.determinant();
    if determinant.abs() > 1e-9 && determinant.is_finite() {
        let columns = matrix.inverse().to_cols_array();
        if columns.iter().all(|v| v.is_finite()) {
            return columns;
        }
    }
    Mat2::IDENTITY.to_cols_array()
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
        // The clip travels through the item's own transform, exactly as the
        // canvas does it, so a scene exercises the same conversion the public
        // API uses rather than a second one written for tests.
        let clip = item.clip.map(|[left, top, right, bottom]| {
            let (min, max) =
                transformed_bounds(&transform, Vec2::new(left, top), Vec2::new(right, bottom));
            Scissor::from_device_bounds(min.into(), max.into(), scene.size)
        });
        // A shape clip is built immediately before the item and stepped back
        // immediately after, so items remain independent: nothing an item
        // clips leaks into the next one.
        let clip_paint = |stencil| Paint {
            material: Material::solid([1.0, 1.0, 1.0, 1.0]),
            blend: BlendMode::Src,
            clip,
            stencil,
        };
        let depth = if let Some(shape) = &item.clip_shape {
            renderer.fill_into(
                batch,
                &shape.to_path(),
                transform,
                &clip_paint(ClipState::narrow(0)),
            )?;
            1
        } else {
            0
        };

        let paint = Paint {
            material: material_for(item, transform, scene.size),
            blend: item.blend,
            clip,
            stencil: ClipState::content(depth),
        };
        match &item.stroke {
            Some(spec) => {
                renderer.stroke_into(batch, &path, &spec.to_style(), transform, &paint)?
            }
            None => renderer.fill_into(batch, &path, transform, &paint)?,
        }

        if depth > 0 {
            let whole = Shape::Rect {
                min: [0.0, 0.0],
                max: [scene.size.width as f32, scene.size.height as f32],
            };
            renderer.fill_into(
                batch,
                &whole.to_path(),
                Affine2::IDENTITY,
                &clip_paint(ClipState::widen(1)),
            )?;
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
