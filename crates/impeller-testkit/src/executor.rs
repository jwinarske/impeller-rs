//! Rendering a scene to an image.
//!
//! Generic over the backend, so the same corpus runs against every one. This is
//! the offscreen executor; WSI and DRM executors render the same scenes through
//! their own presentation paths and compare with the same comparators.
//!
//! A scene is turned into a recording by driving [`Canvas`], not by building a
//! batch by hand. That matters more than it sounds. This file used to resolve
//! gradient endpoints, convert clips to scissors, and step the stencil in and
//! out itself — a second implementation of what the canvas does, which drifted
//! from the first as soon as the scene format grew, and which could not express
//! a layer at all because layers are the canvas's own idea. Going through the
//! canvas means the corpus exercises the code that ships, and a scene can say
//! anything the public API can.

use crate::image::Image;
use crate::scene::{Fill, Item, Node, Scene};
use crate::shape::Shape;
use impeller_core::{
    Canvas, Color, GradientStop, Layer, Paint, Recording, Rect, Shader, Style, Vec2,
};
use impeller_geometry::dash::Dash;
use impeller_hal::{Hal, HalContext, Result};

fn color_of(c: [f32; 4]) -> Color {
    Color::linear(c[0], c[1], c[2], c[3])
}

fn stops_of(stops: &[crate::scene::Stop]) -> Vec<GradientStop> {
    stops
        .iter()
        .map(|stop| GradientStop::new(color_of(stop.color), stop.offset))
        .collect()
}

/// How an item is painted.
///
/// `anti_alias` comes from the scene rather than the item because multisampling
/// is a property of the target: a canvas antialiases everything or nothing, and
/// the scene's sample count is the scene saying which.
fn paint_for(item: &Item, anti_alias: bool) -> Paint {
    let shader = match &item.fill {
        Fill::Solid(color) => Shader::Solid(color_of(*color)),
        Fill::LinearGradient {
            start,
            end,
            stops,
            tile,
        } => Shader::LinearGradient {
            start: Vec2::from(*start),
            end: Vec2::from(*end),
            stops: stops_of(stops),
            tile: *tile,
        },
        Fill::RadialGradient {
            center,
            radius,
            stops,
            tile,
        } => Shader::RadialGradient {
            center: Vec2::from(*center),
            radius: *radius,
            stops: stops_of(stops),
            tile: *tile,
        },
        Fill::SweepGradient {
            center,
            start_angle,
            end_angle,
            stops,
            tile,
        } => Shader::SweepGradient {
            center: Vec2::from(*center),
            start_angle: *start_angle,
            end_angle: *end_angle,
            stops: stops_of(stops),
            tile: *tile,
        },
    };
    Paint {
        shader,
        style: match &item.stroke {
            Some(spec) => Style::Stroke(spec.to_style()),
            None => Style::Fill,
        },
        mask_blur: item.mask_blur,
        dash: item.stroke.as_ref().and_then(|spec| {
            spec.dash
                .as_ref()
                .map(|(intervals, phase)| Dash::new(intervals.clone(), *phase))
        }),
        blend: item.blend,
        anti_alias,
    }
}

fn rect_of([left, top, right, bottom]: [f32; 4]) -> Rect {
    Rect::new(left, top, right, bottom)
}

/// Record one node and everything under it.
///
/// Each node brackets itself with `save`/`restore`, so nothing it does to the
/// transform, the clip or the stencil reaches its siblings. That is what makes
/// a corpus scene a list of independent things rather than a sequence whose
/// meaning depends on what came before.
fn record_node(canvas: &mut Canvas, node: &Node, anti_alias: bool) -> Result<()> {
    match node {
        Node::Draw(item) => {
            canvas.save();
            canvas.concat(item.transform.to_affine());
            if let Some(clip) = item.clip {
                canvas.clip_rect(rect_of(clip))?;
            }
            if let Some(shape) = &item.clip_shape {
                canvas.clip_path(&shape.to_path())?;
            }
            let paint = paint_for(item, anti_alias);
            // A rounded rectangle goes through the call the public API offers
            // for it rather than through its path, so the corpus exercises
            // whichever way that call decides to draw it. Sending the path
            // instead would pin the corpus to the tessellated one and leave the
            // choice untested by everything the corpus drives.
            match &item.shape {
                // A circle goes through its own call for the same reason: that
                // is where the choice between a distance field and four cubics
                // is made, and handing over a path would decide it here.
                Shape::Circle { center, radius } => {
                    canvas.draw_circle(Vec2::from(*center), *radius, &paint)?;
                }
                Shape::Oval { min, max } => {
                    canvas.draw_oval(Rect::new(min[0], min[1], max[0], max[1]), &paint)?;
                }
                Shape::RoundedRect { min, max, radius } => {
                    canvas.draw_rrect(
                        Rect::new(min[0], min[1], max[0], max[1]),
                        *radius,
                        &paint,
                    )?;
                }
                shape => {
                    canvas.draw_path(&shape.to_path(), &paint)?;
                }
            }
            canvas.restore();
        }
        Node::Layer {
            layer,
            bounds,
            transform,
            children,
        } => {
            canvas.save();
            canvas.concat(transform.to_affine());
            let layer = Layer {
                blur: layer.blur,
                alpha: layer.alpha,
                blend: layer.blend,
                backdrop_blur: layer.backdrop_blur,
            };
            match bounds {
                Some(bounds) => canvas.save_layer_bounds(layer, rect_of(*bounds)),
                None => canvas.save_layer(layer),
            };
            for child in children {
                record_node(canvas, child, anti_alias)?;
            }
            canvas.restore();
            canvas.restore();
        }
    }
    Ok(())
}

/// Turn a scene into a recording.
///
/// Shared rather than duplicated per caller: a second copy of this drifted out
/// of step the moment the scene format grew, which is how a frame-loop test
/// ended up reading a field that no longer existed.
pub fn record_scene(scene: &Scene) -> Result<Recording> {
    let mut canvas = Canvas::new(scene.size).with_samples(scene.samples);
    canvas.clear(color_of(scene.background));
    let anti_alias = scene.samples > 1;
    for node in &scene.items {
        record_node(&mut canvas, node, anti_alias)?;
    }
    Ok(canvas.finish())
}

/// Render a scene offscreen and read it back.
pub fn render_scene<H: Hal>(ctx: &mut H::Context, scene: &Scene) -> Result<Image>
where
    H::Context: HalContext<Hal = H>,
{
    let recording = record_scene(scene)?;
    let pixels = impeller_core::render_offscreen::<H>(ctx, &recording, &[])?;
    Ok(Image::new(scene.size.width, scene.size.height, pixels))
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
