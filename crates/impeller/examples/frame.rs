//! Draw one frame and write it out.
//!
//! The thing to run first. It exercises most of what the renderer does — a
//! gradient, a path clip, a group composited through a layer, a blend mode, an
//! image, and a run of glyphs — through the public API alone, naming no backend
//! and reaching for nothing below it.
//!
//! ```sh
//! cargo run -p impeller --example frame -- frame.ppm
//! ```
//!
//! The output is a binary PPM, which every image viewer reads and which needs
//! no encoder. Image encoding is out of scope for this project, and taking a
//! dependency on one so that an example could write a nicer file would be a
//! poor trade — `magick frame.ppm frame.png` converts it, and so does anything
//! else.

use impeller::{
    Atlas, BackendPreference, BlendMode, Canvas, Color, Context, Coverage, Extent2D, GlyphKey,
    GradientStop, Layer, Paint, PathBuilder, PixelFormat, PositionedGlyph, Rect, Vec2,
};

const SIZE: Extent2D = Extent2D {
    width: 512,
    height: 384,
};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "frame.ppm".into());

    // The backend is chosen at run time. One binary serves a board with a
    // working Vulkan driver and one where only GLES is usable, which is the
    // point of the HAL being a trait rather than a build-time choice.
    let mut ctx = match Context::new(BackendPreference::Auto) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("no usable rendering backend: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "{:?} on {}",
        ctx.backend(),
        ctx.capabilities().device_name.clone()
    );

    let (atlas, glyphs) = build_atlas();
    let mut image = ctx
        .create_image(
            Extent2D::new(atlas.size(), atlas.size()),
            PixelFormat::R8Unorm,
        )
        .expect("atlas image");
    ctx.write_image(&mut image, atlas.texels()).expect("upload");

    let mut swatch = ctx
        .create_image(Extent2D::new(2, 2), PixelFormat::Rgba8Unorm)
        .expect("swatch image");
    // Premultiplied, which is what a texture holds. These are opaque, so the
    // two conventions agree — the distinction only shows once alpha is
    // involved, and it is stated here because that is where it gets missed.
    ctx.write_image(
        &mut swatch,
        &[
            220, 90, 60, 255, 60, 140, 220, 255, 60, 200, 140, 255, 230, 200, 70, 255,
        ],
    )
    .expect("upload");

    let recording = compose(&atlas, &glyphs);
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &recording, &[&image, &swatch])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");

    ctx.destroy_surface(surface);
    ctx.destroy_image(swatch);
    ctx.destroy_image(image);

    match write_ppm(&path, SIZE, &pixels) {
        Ok(()) => eprintln!("wrote {path}"),
        Err(e) => {
            eprintln!("writing {path}: {e}");
            std::process::exit(1);
        }
    }
}

/// Everything the frame draws.
fn compose(atlas: &Atlas, glyphs: &[PositionedGlyph]) -> impeller::Recording {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::rgba8(18, 20, 28, 255));

    // A gradient, whose endpoints travel through the transform with the shape
    // rather than staying pinned to the screen.
    canvas
        .draw_rect(
            Rect::new(32.0, 32.0, 480.0, 160.0),
            &Paint::linear_gradient(
                Vec2::new(32.0, 32.0),
                Vec2::new(480.0, 160.0),
                vec![
                    GradientStop::new(Color::rgba8(40, 70, 160, 255), 0.0),
                    GradientStop::new(Color::rgba8(180, 60, 120, 255), 0.55),
                    GradientStop::new(Color::rgba8(240, 180, 80, 255), 1.0),
                ],
            ),
        )
        .expect("gradient");

    // A group composited at reduced opacity. Drawn directly, the two circles
    // would show where they overlap; through a layer they do not, because the
    // group is made first and faded once.
    canvas.save_layer(Layer::opacity(0.65));
    for (x, color) in [(180.0, (230, 90, 70)), (240.0, (70, 200, 160))] {
        canvas
            .draw_circle(
                Vec2::new(x, 108.0),
                52.0,
                &Paint::fill(Color::rgba8(color.0, color.1, color.2, 255)),
            )
            .expect("circle");
    }
    canvas.restore();

    // A path clip, which no rectangle expresses and which the stencil carries.
    canvas.save();
    let mut triangle = PathBuilder::new();
    triangle
        .move_to(Vec2::new(256.0, 196.0))
        .line_to(Vec2::new(400.0, 340.0))
        .line_to(Vec2::new(112.0, 340.0))
        .close();
    canvas.clip_path(&triangle.build()).expect("clip");
    canvas
        .draw_rect(
            Rect::new(96.0, 190.0, 416.0, 348.0),
            &Paint::image(1, Rect::new(96.0, 190.0, 416.0, 348.0)),
        )
        .expect("image");
    canvas.restore();

    // A blend mode over the top. Plus accumulates rather than replacing, which
    // is what makes the overlap brighten.
    canvas
        .draw_circle(
            Vec2::new(430.0, 300.0),
            60.0,
            &Paint::fill(Color::rgba8(90, 60, 140, 255)).with_blend(BlendMode::Plus),
        )
        .expect("glow");

    canvas
        .draw_glyphs(glyphs, atlas, 0, &Paint::fill(Color::WHITE))
        .expect("glyphs");

    canvas.finish()
}

/// An atlas of blocks standing in for rasterized glyphs, and a run using them.
///
/// Rasterizing an outline needs a font parser, which is out of scope — bring
/// `swash` or `ttf-parser` and hand the coverage over. What this shows is the
/// shape of that handover and that a run of any length is a single draw.
fn build_atlas() -> (Atlas, Vec<PositionedGlyph>) {
    let mut atlas = Atlas::new(64);
    let mut run = Vec::new();
    for i in 0..8u16 {
        let key = GlyphKey {
            font: 1,
            glyph: i,
            size: 24,
        };
        // A wedge per glyph, so the run is visibly a sequence of different
        // shapes rather than one repeated block.
        let (w, h) = (10u32, 18u32);
        let mut texels = vec![0u8; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let lit = x as f32 / w as f32 + y as f32 / h as f32 > i as f32 / 8.0;
                texels[(y * w + x) as usize] = if lit { 255 } else { 40 };
            }
        }
        let rect = atlas
            .insert(
                key,
                &Coverage {
                    width: w,
                    height: h,
                    texels,
                },
            )
            .expect("atlas insert");
        run.push(PositionedGlyph::new(
            key,
            [40.0 + i as f32 * 14.0, 356.0],
            rect,
        ));
    }
    (atlas, run)
}

/// Write tightly packed RGBA as a binary PPM.
///
/// Alpha is dropped, which is what the format holds. The surface was cleared
/// opaque, so nothing here depends on how a viewer would have composited it.
fn write_ppm(path: &str, extent: Extent2D, pixels: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(out, "P6\n{} {}\n255\n", extent.width, extent.height)?;
    let mut rgb = Vec::with_capacity(pixels.len() / 4 * 3);
    for texel in pixels.chunks_exact(4) {
        rgb.extend_from_slice(&texel[..3]);
    }
    out.write_all(&rgb)?;
    out.flush()
}
