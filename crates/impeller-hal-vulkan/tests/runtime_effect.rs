//! A fragment program this renderer did not know about, drawn.
//!
//! What a runtime effect is, once the premise is right: not a shader compiled
//! at run time, but a pipeline built at run time from a module supplied by
//! somebody else. So the thing worth testing is the pipeline path -- that a
//! program can be registered, that a draw naming it gets its own pipeline, and
//! that the picture is the caller's rather than any the built-in shader could
//! have produced.
//!
//! The program comes from this workspace's own build, which translates WGSL to
//! both backends' payloads. That is the same arrangement a caller has to make,
//! so the test exercises the real path rather than a special case for it.

use impeller_hal::{
    Batch, BlendMode, Extent2D, Material, PassDescriptor, PixelFormat, RuntimeProgram,
    TextureDescriptor,
};
use impeller_hal_vulkan::{DevicePreference, Validated};

const SIZE: Extent2D = Extent2D {
    width: 32,
    height: 32,
};
const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

fn context() -> Option<Validated> {
    Validated::new(DevicePreference::Auto).ok()
}

/// The uniform block, as the effect reads it: two colours and a threshold.
fn uniforms(left: [f32; 4], right: [f32; 4], threshold: f32) -> Vec<f32> {
    let mut out = vec![0.0; impeller_hal::RUNTIME_FLOATS];
    out[0..4].copy_from_slice(&left);
    out[4..8].copy_from_slice(&right);
    // Where `geometry` sits in the block, which is what the effect reads its
    // threshold from.
    out[20] = threshold;
    out
}

fn render(ctx: &mut Validated, material: Material) -> Vec<u8> {
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, material, BlendMode::Src)
        .expect("push");
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch(
        &mut target,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);
    pixels
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE.width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

fn program() -> RuntimeProgram {
    RuntimeProgram {
        spirv: impeller_shaders::EFFECT_SPV.to_vec(),
        glsl_es: impeller_shaders::EFFECT_FS_GLSL.to_string(),
    }
}

#[test]
fn a_registered_program_draws_its_own_picture() {
    let Some(mut ctx) = context() else { return };
    let id = ctx.register_program(&program()).expect("register");

    // A vertical split at the middle of clip space, which no material here
    // draws: the closest is a linear gradient, and that has no hard edge.
    let pixels = render(
        &mut ctx,
        Material::Runtime {
            program: id,
            uniforms: uniforms([1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], 0.0),
        },
    );
    assert_eq!(pixel(&pixels, 4, 16), [255, 0, 0, 255], "left of the split");
    assert_eq!(pixel(&pixels, 28, 16), [0, 0, 255, 255], "right of it");
}

#[test]
fn the_uniforms_a_caller_packed_are_the_ones_the_program_reads() {
    // The interface, checked rather than assumed: an effect reads the paint's
    // own block, so moving the threshold has to move the split. A program
    // whose uniforms never arrived would draw the same picture at every value.
    let Some(mut ctx) = context() else { return };
    let id = ctx.register_program(&program()).expect("register");

    let at = |ctx: &mut Validated, threshold: f32| {
        render(
            ctx,
            Material::Runtime {
                program: id,
                uniforms: uniforms([1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], threshold),
            },
        )
    };

    // Three quarters of the way across. Clip space runs from minus one to one
    // over the target's width, so a threshold of a half falls at pixel
    // twenty-four of thirty-two -- which is why the samples are either side of
    // that and not either side of the middle.
    let moved = at(&mut ctx, 0.5);
    assert_eq!(
        pixel(&moved, 20, 16),
        [255, 0, 0, 255],
        "the split should have moved with the uniform"
    );
    assert_eq!(pixel(&moved, 28, 16), [0, 0, 255, 255], "and not past it");
}

#[test]
fn a_program_nobody_registered_is_refused_rather_than_drawn() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(
            &FULL,
            &QUAD,
            Material::Runtime {
                program: 7,
                uniforms: uniforms([1.0; 4], [0.0; 4], 0.0),
            },
            BlendMode::Src,
        )
        .expect("push");
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    let result = ctx.submit_batch(&mut target, &batch, PassDescriptor::clear([0.0; 4]));
    ctx.destroy_texture(target);
    assert!(
        result.is_err(),
        "naming a program nobody registered should be refused, not drawn with whatever is at that index"
    );
}

#[test]
fn a_built_in_material_still_draws_beside_a_registered_one() {
    // The pipeline cache now holds more than one program, and the built-in one
    // has to keep working -- both in the same batch, where the two pipelines
    // are bound in turn.
    let Some(mut ctx) = context() else { return };
    let id = ctx.register_program(&program()).expect("register");

    let mut batch = Batch::new();
    batch
        .push(
            &FULL,
            &QUAD,
            Material::Runtime {
                program: id,
                uniforms: uniforms([1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], 0.0),
            },
            BlendMode::Src,
        )
        .expect("effect");
    // A solid over the right half, drawn by the built-in shader.
    batch
        .push(
            &[[0.0, -1.0], [1.0, -1.0], [1.0, 1.0], [0.0, 1.0]],
            &QUAD,
            Material::solid([0.0, 1.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("solid");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch(
        &mut target,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);

    assert_eq!(pixel(&pixels, 4, 16), [255, 0, 0, 255], "the effect's half");
    assert_eq!(pixel(&pixels, 24, 16), [0, 255, 0, 255], "the built-in one");
}
