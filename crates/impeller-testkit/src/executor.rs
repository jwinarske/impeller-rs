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
    Affine2, Canvas, Color, GradientStop, Layer, Paint, Recording, Rect, Shader, SourceRect,
    Sprite, Style, Vec2, Vertices,
};
use impeller_geometry::dash::Dash;
use impeller_hal::{Hal, HalContext, PixelFormat, Result, TextureDescriptor};

fn color_of(c: [f32; 4]) -> Color {
    Color::linear(c[0], c[1], c[2], c[3])
}

fn stops_of(stops: &[crate::scene::Stop]) -> Vec<GradientStop> {
    stops
        .iter()
        .map(|stop| GradientStop::new(color_of(stop.color), stop.offset))
        .collect()
}

/// The shader half of a paint.
///
/// Separate from the rest because a node that is not an item has a fill and
/// nothing else to say: no stroke, no dash, no mask blur, no colour filter.
/// A mesh is the case that needed it.
fn shader_for(fill: &Fill) -> Shader {
    match fill {
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
        Fill::RuntimeEffect { uniforms } => Shader::RuntimeEffect {
            // Program zero, always: a scene names no program, and the
            // executor registers exactly one. Registering is idempotent, so
            // this is the same index every time whatever else a caller has
            // registered before it -- provided they registered this one first,
            // which the executor does.
            program: 0,
            uniforms: uniforms.clone(),
            // The scene format has no way to say a program samples the sheet
            // yet, and the fixture effect does not. Adding it is the same
            // shape of change as the fill that named the sheet, for the day a
            // plate needs one.
            image: None,
        },
        Fill::Image {
            rect,
            source,
            tile,
            sampling,
            alpha,
            tint,
        } => Shader::Image {
            // Slot zero, always: a scene names no textures, and the executor
            // supplies exactly one.
            slot: crate::fixture::SLOT,
            rect: Rect::new(rect[0], rect[1], rect[2], rect[3]),
            alpha: *alpha,
            tile: *tile,
            source: Rect::new(source[0], source[1], source[2], source[3]),
            tint: color_of(*tint),
            sampling: *sampling,
        },
        Fill::ConicalGradient {
            start_center,
            start_radius,
            end_center,
            end_radius,
            stops,
            tile,
        } => Shader::ConicalGradient {
            start_center: Vec2::from(*start_center),
            start_radius: *start_radius,
            end_center: Vec2::from(*end_center),
            end_radius: *end_radius,
            stops: stops_of(stops),
            tile: *tile,
        },
    }
}

/// A paint from a fill alone, for the nodes that have no item to speak for
/// them.
fn paint_from(fill: &Fill, anti_alias: bool) -> Paint {
    Paint {
        shader: shader_for(fill),
        anti_alias,
        ..Paint::default()
    }
}

/// How an item is painted.
///
/// `anti_alias` comes from the scene rather than the item because multisampling
/// is a property of the target: a canvas antialiases everything or nothing, and
/// the scene's sample count is the scene saying which.
fn paint_for(item: &Item, anti_alias: bool) -> Paint {
    let shader = shader_for(&item.fill);
    Paint {
        shader,
        color_filter: item.color_filter,
        // Not described by the corpus, and for a reason the color filter's
        // presence there makes clearer by contrast. A color filter is
        // arithmetic in the fragment shader, which the two backends reach by
        // separate translations and can disagree about. An image filter is a
        // layer opened and composited, which produces the same recording
        // whatever draws it -- and the passes that recording contains are
        // already compared, by the scenes that blur a layer directly.
        image_filter: impeller_core::ImageFilter::None,
        style: match &item.stroke {
            Some(spec) => Style::Stroke(spec.to_style()),
            None => Style::Fill,
        },
        mask_blur: item.mask_blur,
        mask_blur_style: item.mask_blur_style,
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
        Node::Mesh(mesh) => {
            canvas.save();
            canvas.concat(mesh.transform.to_affine());
            let vertices = Vertices::full(
                mesh.mode,
                mesh.positions.iter().copied().map(Vec2::from).collect(),
                mesh.texture_coords
                    .iter()
                    .copied()
                    .map(Vec2::from)
                    .collect(),
                mesh.colors.iter().copied().map(color_of).collect(),
                if mesh.indices.is_empty() {
                    (0..mesh.positions.len() as u32).collect()
                } else {
                    mesh.indices.clone()
                },
            )?;
            let mut paint = paint_from(&mesh.fill, anti_alias);
            paint.blend = mesh.blend;
            // Restored before the error is raised, or a mesh a device refuses
            // would leave the canvas inside a save nobody closes and every
            // later node in the scene inside it too.
            let result = canvas.draw_vertices(&vertices, &paint).map(|_| ());
            canvas.restore();
            result?;
        }
        Node::Atlas(atlas) => {
            canvas.save();
            let sprites: Vec<Sprite> = atlas
                .sprites
                .iter()
                .map(|s| {
                    Sprite::new(
                        SourceRect::new(
                            s.source[0],
                            s.source[1],
                            s.source[2] - s.source[0],
                            s.source[3] - s.source[1],
                        ),
                        Affine2::from_scale_angle_translation(
                            Vec2::splat(s.scale),
                            s.rotate,
                            Vec2::from(s.translate),
                        ),
                    )
                    .with_color(color_of(s.color))
                })
                .collect();
            // The paint's own rectangle is never mapped through for a sprite
            // batch -- each sprite's source rectangle says what it reads --
            // so this only has to be a well-formed one.
            let paint = Paint::image(crate::fixture::SLOT, Rect::new(0.0, 0.0, 1.0, 1.0))
                .with_image_alpha(atlas.alpha)
                .with_blend(atlas.blend);
            let result = canvas
                .draw_atlas(&sprites, crate::fixture::SIZE, &paint)
                .map(|_| ());
            canvas.restore();
            result?;
        }
        Node::Shadow(shadow) => {
            canvas.save();
            canvas.concat(shadow.transform.to_affine());
            let path = shadow.shape.to_path();
            let mut result = canvas
                .draw_shadow(
                    &path,
                    color_of(shadow.color),
                    shadow.elevation,
                    shadow.transparent_occluder,
                )
                .map(|_| ());
            if result.is_ok() && shadow.with_caster {
                // The object itself, which is what makes the shadow legible:
                // an outer shadow with nothing on top is a picture of a hole.
                result = canvas
                    .draw_path(&path, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
                    .map(|_| ());
            }
            canvas.restore();
            result?;
        }
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
    // Uploaded per scene that asks for it rather than held by the caller,
    // which keeps every consumer of this function -- the window, the
    // comparison, the sheet renderer -- from having to know that some scenes
    // sample a texture. A scene that does not ask allocates nothing.
    // Registered before anything is drawn and before the sheet, so the index
    // the scenes name is the one it gets. Idempotent, so a list of scenes
    // costs one program rather than one per scene.
    if scene.uses_effect() {
        let id = ctx.register_program(&crate::fixture::effect())?;
        if id != 0 {
            return Err(impeller_hal::Error::Unsupported(
                "the fixture effect was not the first program registered with this context",
            ));
        }
    }
    let pixels = if scene.samples_fixture() {
        let mut sheet = ctx.create_texture(&TextureDescriptor::offscreen(
            crate::fixture::SIZE,
            PixelFormat::Rgba8Unorm,
        ))?;
        let result = ctx
            .write_texture(&mut sheet, &crate::fixture::pixels())
            .and_then(|()| impeller_core::render_offscreen::<H>(ctx, &recording, &[&sheet]));
        // Released whether the draw worked or not: a scene that fails to
        // render must not leak a texture into every later scene's device.
        ctx.destroy_texture(sheet);
        result?
    } else {
        impeller_core::render_offscreen::<H>(ctx, &recording, &[])?
    };
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
