//! Textures, programs, and replaying a batch as GL calls.
//!
//! This is where the batch-shaped HAL earns its keep. GL has one global state
//! machine, so the cost that matters is redundant state changes rather than
//! command encoding. Having the whole batch up front means blend state is set
//! only where it actually changes, and the geometry for every draw is uploaded
//! in one buffer rather than one per draw.

use crate::context::GlesContext;
use glow::HasContext;
use impeller_hal::{
    Batch, BlendMode, Error, Extent2D, PassDescriptor, PixelFormat, Result, TextureDescriptor,
};

/// A colour texture and the framebuffer that renders into it.
pub struct GlesTexture {
    pub(crate) texture: glow::Texture,
    pub(crate) framebuffer: glow::Framebuffer,
    pub(crate) extent: Extent2D,
    pub(crate) format: PixelFormat,
}

impl GlesTexture {
    pub fn extent(&self) -> Extent2D {
        self.extent
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }
}

/// The compiled solid-colour program and the vertex state it draws with.
pub(crate) struct SolidProgram {
    pub(crate) program: glow::Program,
    /// Locations of the paint uniform's members, which the shader translator
    /// lowered the push constant to. Looked up by name, so those names are part
    /// of the contract rather than an implementation detail.
    ///
    /// GLES has no push constants and no way to set a struct in one call, so
    /// each member is set individually. The array of stops needs one location
    /// per element for the same reason.
    pub(crate) stops: [Option<glow::UniformLocation>; impeller_hal::MAX_STOPS],
    /// Every non-array member, paired with where it starts in the packed
    /// material.
    ///
    /// Pairing them means the offsets come from the shared layout rather than
    /// being written out again as bare indices, which is how this fell out of
    /// step when the material grew a matrix.
    pub(crate) members: Vec<(Option<glow::UniformLocation>, usize)>,
    pub(crate) vao: glow::VertexArray,
    pub(crate) vertices: glow::Buffer,
    pub(crate) indices: glow::Buffer,
}

impl GlesContext {
    /// Allocate a texture and a framebuffer that renders into it.
    pub fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<GlesTexture> {
        if desc.is_external() {
            return Err(Error::Unsupported("external image import"));
        }
        if desc.extent.is_empty() {
            return Err(Error::Unsupported("zero-sized texture"));
        }
        if !self.capabilities().can_allocate(desc.extent) {
            return Err(Error::LimitExceeded {
                what: "texture dimension",
                requested: desc.extent.width.max(desc.extent.height) as u64,
                limit: self.capabilities().max_texture_size as u64,
            });
        }

        let gl = self.raw_gl();
        // SAFETY: a context is current on this thread for the context's life.
        unsafe {
            let texture = gl
                .create_texture()
                .map_err(|e| gl_err("create_texture", &e))?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_storage_2d(
                glow::TEXTURE_2D,
                1,
                internal_format(desc.format),
                desc.extent.width as i32,
                desc.extent.height as i32,
            );
            // Sized storage with one level, so filtering must not expect mips.
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );

            let framebuffer = gl
                .create_framebuffer()
                .map_err(|e| gl_err("create_framebuffer", &e))?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );

            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(framebuffer);
                gl.delete_texture(texture);
                return Err(Error::Backend {
                    backend: "gles",
                    detail: format!("framebuffer incomplete: {status:#x}"),
                });
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);

            Ok(GlesTexture {
                texture,
                framebuffer,
                extent: desc.extent,
                format: desc.format,
            })
        }
    }

    pub fn destroy_texture(&mut self, texture: GlesTexture) {
        let gl = self.raw_gl();
        // SAFETY: the caller has given up the texture, and GL calls are
        // sequential on the one thread holding the context.
        unsafe {
            gl.delete_framebuffer(texture.framebuffer);
            gl.delete_texture(texture.texture);
        }
    }

    /// Draw a batch into a target.
    pub fn submit_batch(
        &mut self,
        target: &mut GlesTexture,
        batch: &Batch,
        pass: PassDescriptor,
    ) -> Result<()> {
        if !pass.samples.is_power_of_two() {
            return Err(Error::Unsupported("sample count is not a power of two"));
        }
        if !self.capabilities().sample_counts.supports(pass.samples) {
            return Err(Error::Unsupported("sample count not supported by device"));
        }
        if pass.is_multisampled() && pass.clear.is_none() {
            // Seeding the multisample buffer from the target would need a blit
            // from single-sample to multisample, which is not a legal blit.
            // Refusing matches the Vulkan backend, which cannot do it either,
            // so the restriction is a property of the technique rather than of
            // one backend.
            return Err(Error::Unsupported(
                "a multisampled pass must clear; preserving needs a single-to-multisample copy",
            ));
        }
        self.ensure_program()?;

        let extent = target.extent;
        // Rendering goes to a transient multisample framebuffer and is resolved
        // into the target afterwards, so the target stays single-sampled and
        // directly readable, exactly as on the Vulkan side.
        let multisample = if pass.is_multisampled() {
            Some(self.create_multisample_target(extent, target.format, pass.samples)?)
        } else {
            None
        };
        let render_fbo = multisample
            .as_ref()
            .map_or(target.framebuffer, |ms| ms.framebuffer);

        let gl = self.raw_gl();
        // SAFETY: a context is current, and every object bound below was
        // created by this context.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(render_fbo));
            gl.viewport(0, 0, extent.width as i32, extent.height as i32);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);

            if let Some(color) = pass.clear {
                gl.clear_color(color[0], color[1], color[2], color[3]);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }

            if batch.is_empty() {
                resolve_and_unbind(gl, &multisample, target, extent);
                if let Some(ms) = multisample {
                    ms.destroy(gl);
                }
                return Ok(());
            }

            let program = self.program().expect("ensured above");
            gl.use_program(Some(program.program));
            gl.bind_vertex_array(Some(program.vao));

            gl.bind_buffer(glow::ARRAY_BUFFER, Some(program.vertices));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                cast_bytes(batch.vertices()),
                glow::STREAM_DRAW,
            );
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(program.indices));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                cast_bytes(batch.indices()),
                glow::STREAM_DRAW,
            );

            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);

            // Blend state is global, so it is set only where a draw actually
            // needs a different mode. Tracking it here is the direct analogue
            // of binding a pipeline only on change.
            let mut current: Option<BlendMode> = None;
            for draw in batch.draws() {
                if current != Some(draw.blend) {
                    apply_blend(gl, draw.blend);
                    current = Some(draw.blend);
                }
                let packed = draw.material.to_push_constants();
                let set = |location: &Option<glow::UniformLocation>, at: usize| {
                    if let Some(location) = location {
                        gl.uniform_4_f32(
                            Some(location),
                            packed[at],
                            packed[at + 1],
                            packed[at + 2],
                            packed[at + 3],
                        );
                    }
                };
                for (i, location) in program.stops.iter().enumerate() {
                    set(location, impeller_hal::material::layout::STOPS + i * 4);
                }
                for (location, at) in &program.members {
                    set(location, *at);
                }
                gl.draw_elements(
                    glow::TRIANGLES,
                    draw.index_count as i32,
                    glow::UNSIGNED_INT,
                    // Byte offset into the index buffer, not an index count.
                    (draw.first_index * 4) as i32,
                );
            }

            gl.disable(glow::BLEND);
            gl.bind_vertex_array(None);
            resolve_and_unbind(gl, &multisample, target, extent);
            if let Some(ms) = multisample {
                ms.destroy(gl);
            }

            let error = gl.get_error();
            if error != glow::NO_ERROR {
                return Err(Error::Backend {
                    backend: "gles",
                    detail: format!("draw produced GL error {error:#x}"),
                });
            }
        }
        Ok(())
    }

    /// Read a target back, tightly packed and top-row first.
    pub fn read_texture(&mut self, texture: &mut GlesTexture) -> Result<Vec<u8>> {
        let extent = texture.extent;
        let size = (extent.area() * 4) as usize;
        let mut pixels = vec![0u8; size];

        let gl = self.raw_gl();
        // SAFETY: the framebuffer is complete and the destination is sized for
        // the region being read.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(texture.framebuffer));
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                0,
                0,
                extent.width as i32,
                extent.height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(&mut pixels),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);

            let error = gl.get_error();
            if error != glow::NO_ERROR {
                return Err(Error::Backend {
                    backend: "gles",
                    detail: format!("readback produced GL error {error:#x}"),
                });
            }
        }

        // No row flip, despite GL numbering framebuffer rows from the bottom.
        // The shader translator already negates Y when targeting GLSL, which
        // cancels the origin difference exactly: a read comes back in the same
        // orientation as every other backend. Flipping here as well would undo
        // that and mirror the image, which is what the orientation tests catch.
        Ok(pixels)
    }

    /// Allocate a transient multisample framebuffer matching a target.
    fn create_multisample_target(
        &self,
        extent: Extent2D,
        format: PixelFormat,
        samples: u32,
    ) -> Result<MultisampleTarget> {
        let gl = self.raw_gl();
        // SAFETY: a context is current; every object is deleted on the failure
        // paths below.
        unsafe {
            let renderbuffer = gl
                .create_renderbuffer()
                .map_err(|e| gl_err("create_renderbuffer", &e))?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(renderbuffer));
            gl.renderbuffer_storage_multisample(
                glow::RENDERBUFFER,
                samples as i32,
                internal_format(format),
                extent.width as i32,
                extent.height as i32,
            );

            let framebuffer = match gl.create_framebuffer() {
                Ok(fb) => fb,
                Err(e) => {
                    gl.delete_renderbuffer(renderbuffer);
                    return Err(gl_err("create_framebuffer", &e));
                }
            };
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(renderbuffer),
            );

            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(framebuffer);
                gl.delete_renderbuffer(renderbuffer);
                return Err(Error::Backend {
                    backend: "gles",
                    detail: format!("multisample framebuffer incomplete: {status:#x}"),
                });
            }

            Ok(MultisampleTarget {
                framebuffer,
                renderbuffer,
            })
        }
    }

    fn ensure_program(&mut self) -> Result<()> {
        if self.program().is_some() {
            return Ok(());
        }
        let built = build_program(self.raw_gl())?;
        self.set_program(built);
        Ok(())
    }
}

/// A transient multisample framebuffer, resolved into a target and discarded.
struct MultisampleTarget {
    framebuffer: glow::Framebuffer,
    renderbuffer: glow::Renderbuffer,
}

impl MultisampleTarget {
    /// # Safety
    ///
    /// A context must be current and the objects must belong to it.
    unsafe fn destroy(&self, gl: &glow::Context) {
        gl.delete_framebuffer(self.framebuffer);
        gl.delete_renderbuffer(self.renderbuffer);
    }
}

/// Resolve a multisample framebuffer into the target, then unbind.
///
/// # Safety
///
/// A context must be current and both framebuffers must be complete.
unsafe fn resolve_and_unbind(
    gl: &glow::Context,
    multisample: &Option<MultisampleTarget>,
    target: &GlesTexture,
    extent: Extent2D,
) {
    if let Some(ms) = multisample {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(ms.framebuffer));
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(target.framebuffer));
        // NEAREST, not LINEAR: a blit whose read buffer is multisampled and
        // whose draw buffer is not must use NEAREST, and the resolve itself is
        // what averages the samples.
        gl.blit_framebuffer(
            0,
            0,
            extent.width as i32,
            extent.height as i32,
            0,
            0,
            extent.width as i32,
            extent.height as i32,
            glow::COLOR_BUFFER_BIT,
            glow::NEAREST,
        );
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
    }
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
}

fn apply_blend(gl: &glow::Context, blend: BlendMode) {
    // SAFETY: a context is current.
    unsafe {
        match blend {
            BlendMode::Src => gl.disable(glow::BLEND),
            BlendMode::SrcOver => {
                gl.enable(glow::BLEND);
                // ONE rather than SRC_ALPHA because the shader emits
                // premultiplied colour, exactly as on the Vulkan side.
                gl.blend_func_separate(
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                );
                gl.blend_equation(glow::FUNC_ADD);
            }
        }
    }
}

fn build_program(gl: &glow::Context) -> Result<SolidProgram> {
    // SAFETY: a context is current; every object is deleted on the failure
    // paths below.
    unsafe {
        let program = gl
            .create_program()
            .map_err(|e| gl_err("create_program", &e))?;

        let mut shaders = Vec::new();
        for (stage, source) in [
            (glow::VERTEX_SHADER, impeller_shaders::SOLID_VS_GLSL),
            (glow::FRAGMENT_SHADER, impeller_shaders::SOLID_FS_GLSL),
        ] {
            let shader = match gl.create_shader(stage) {
                Ok(s) => s,
                Err(e) => {
                    cleanup(gl, program, &shaders);
                    return Err(gl_err("create_shader", &e));
                }
            };
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let log = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                cleanup(gl, program, &shaders);
                // A compile failure means the translated source is wrong, which
                // is a build-time problem surfacing at runtime; the log is the
                // only way to tell which line.
                return Err(Error::Backend {
                    backend: "gles",
                    detail: format!("shader compilation failed: {log}"),
                });
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }

        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            cleanup(gl, program, &shaders);
            return Err(Error::Backend {
                backend: "gles",
                detail: format!("program link failed: {log}"),
            });
        }
        for shader in &shaders {
            gl.detach_shader(program, *shader);
            gl.delete_shader(*shader);
        }

        // The translator lowers the push constant to a uniform struct, so each
        // member is addressed by its qualified name. A member the compiler
        // decided was unused has no location, which is why these are optional
        // rather than an error.
        let mut stops = [const { None }; impeller_hal::MAX_STOPS];
        for (i, slot) in stops.iter_mut().enumerate() {
            *slot =
                gl.get_uniform_location(program, &format!("_push_constant_binding_fs.stops[{i}]"));
        }
        // Name and offset together, so adding a member to the material means
        // adding one line here rather than editing indices in two places.
        use impeller_hal::material::layout;
        let members: Vec<(Option<glow::UniformLocation>, usize)> = [
            ("offsets", layout::OFFSETS),
            ("geometry", layout::GEOMETRY),
            ("to_local", layout::TO_LOCAL),
            ("params", layout::PARAMS),
        ]
        .into_iter()
        .map(|(name, at)| {
            let qualified = format!("_push_constant_binding_fs.{name}");
            (gl.get_uniform_location(program, &qualified), at)
        })
        .collect();

        let vao = gl
            .create_vertex_array()
            .map_err(|e| gl_err("create_vertex_array", &e))?;
        let vertices = gl
            .create_buffer()
            .map_err(|e| gl_err("create_buffer", &e))?;
        let indices = gl
            .create_buffer()
            .map_err(|e| gl_err("create_buffer", &e))?;

        Ok(SolidProgram {
            program,
            stops,
            members,
            vao,
            vertices,
            indices,
        })
    }
}

/// Delete a partially built program.
///
/// # Safety
///
/// A context must be current and the objects must belong to it.
unsafe fn cleanup(gl: &glow::Context, program: glow::Program, shaders: &[glow::Shader]) {
    for shader in shaders {
        gl.delete_shader(*shader);
    }
    gl.delete_program(program);
}

fn internal_format(format: PixelFormat) -> u32 {
    match format {
        PixelFormat::Rgba8Unorm => glow::RGBA8,
        PixelFormat::Rgba8UnormSrgb => glow::SRGB8_ALPHA8,
        // GLES has no BGRA sized internal format; storage is RGBA8 and the
        // channel order is a readback concern rather than an allocation one.
        PixelFormat::Bgra8Unorm => glow::RGBA8,
        PixelFormat::Bgra8UnormSrgb => glow::SRGB8_ALPHA8,
        PixelFormat::Rgb10A2Unorm => glow::RGB10_A2,
        PixelFormat::Rgba16Float => glow::RGBA16F,
    }
}

fn gl_err(what: &str, detail: &str) -> Error {
    Error::Backend {
        backend: "gles",
        detail: format!("{what}: {detail}"),
    }
}

fn cast_bytes<T>(slice: &[T]) -> &[u8] {
    // SAFETY: T is plain data here ([f32; 2] or u32), fully initialized, and
    // the result borrows the same memory for no longer.
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn casting_to_bytes_preserves_length() {
        let vertices = [[0.0f32, 1.0], [2.0, 3.0]];
        assert_eq!(cast_bytes(&vertices).len(), 16);
        let indices = [0u32, 1, 2];
        assert_eq!(cast_bytes(&indices).len(), 12);
    }
}
