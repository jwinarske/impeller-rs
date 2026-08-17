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
    Batch, BlendMode, ClipRole, ClipState, Error, Extent2D, PassDescriptor, PixelFormat, Result,
    Scissor, TextureDescriptor,
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
    /// The sampler the image paint reads.
    ///
    /// The translator combines WGSL's separate texture and sampler into one
    /// GLSL sampler, named for the texture's group and binding, so that name is
    /// part of the contract in exactly the way the paint member names are.
    pub(crate) image: Option<glow::UniformLocation>,
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
            // Clamped, to match the Vulkan sampler. GL defaults to repeating,
            // and the difference is invisible until something samples an edge:
            // a linear filter at the last texel then blends with the first one
            // from the opposite side, where clamping holds the edge. Tiling is
            // the shader's job, so a texture that wrapped here would tile twice
            // in repeat mode and bleed in the other two.
            for axis in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, axis, glow::CLAMP_TO_EDGE as i32);
            }

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
        self.submit_batch_textured(target, batch, pass, &[])
    }

    /// Draw a batch into a target, with a texture table its image paints index.
    pub fn submit_batch_textured(
        &mut self,
        target: &mut GlesTexture,
        batch: &Batch,
        pass: PassDescriptor,
        textures: &[&GlesTexture],
    ) -> Result<()> {
        // Checked before anything is bound, so a batch naming a slot nobody
        // supplied fails rather than reading whatever is on the unit.
        for slot in batch.texture_slots() {
            if slot as usize >= textures.len() {
                return Err(Error::Backend {
                    backend: "gles",
                    detail: format!(
                        "a draw samples texture slot {slot}, but only {} were supplied",
                        textures.len()
                    ),
                });
            }
        }
        let placeholder = self.placeholder_texture()?;
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
        self.capabilities().check_blend_modes(batch)?;
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
            // Attached to whichever framebuffer is being drawn into, for the
            // duration of the pass only. The target's framebuffer outlives the
            // pass, so this is detached again at the end rather than left on it
            // -- a later pass that clips nothing should not be paying for a
            // stencil buffer it never reads.
            let stencil = if batch.uses_stencil() {
                match attach_stencil(gl, extent, pass.samples) {
                    Ok(renderbuffer) => Some(renderbuffer),
                    Err(e) => {
                        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                        if let Some(ms) = multisample {
                            ms.destroy(gl);
                        }
                        return Err(e);
                    }
                }
            } else {
                None
            };
            gl.viewport(0, 0, extent.width as i32, extent.height as i32);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::STENCIL_TEST);
            // Writing is enabled unconditionally before the clear, because a
            // stencil clear is masked the same way a draw is: with the mask at
            // zero the buffer keeps whatever the renderbuffer was allocated
            // with, and every clip then tests against garbage.
            gl.stencil_mask(0xff);
            gl.color_mask(true, true, true, true);

            if let Some(color) = pass.clear {
                gl.clear_color(color[0], color[1], color[2], color[3]);
                gl.clear_stencil(0);
                // Clearing the stencil alongside the color even where there is
                // no stencil attachment is harmless and keeps the two paths
                // from differing in anything but the attachment itself.
                gl.clear(glow::COLOR_BUFFER_BIT | glow::STENCIL_BUFFER_BIT);
            }

            if batch.is_empty() {
                detach_stencil(gl, render_fbo, stencil);
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

            // Blend and scissor state are both global, so each is set only
            // where a draw actually needs a different one. Tracking them here
            // is the direct analogue of binding a pipeline only on change.
            let mut current: Option<BlendMode> = None;
            // `None` means the scissor test is off, which is how the pass
            // started and what an unclipped draw wants.
            let mut scissor: Option<Scissor> = None;
            let mut stencil_state: Option<ClipState> = None;
            let mut bound_texture: Option<Option<u32>> = None;
            for draw in batch.draws() {
                if current != Some(draw.blend) {
                    apply_blend(gl, draw.blend);
                    current = Some(draw.blend);
                }
                if stencil.is_some() && stencil_state != Some(draw.stencil) {
                    apply_stencil(gl, draw.stencil);
                    stencil_state = Some(draw.stencil);
                }
                // A clip covering the whole target is the same as none, and
                // saying so keeps a run of unclipped draws from toggling the
                // test on and off around them.
                let wanted = draw
                    .clip
                    .map(|clip| clip.clamped_to(extent))
                    .filter(|clip| !clip.covers(extent));
                if scissor != wanted {
                    match wanted {
                        Some(clip) => {
                            // No vertical conversion, despite GL numbering
                            // window rows upward from the bottom. The shader
                            // translator already negates Y for GLSL, which puts
                            // the image's top row at GL's y of zero -- the same
                            // cancellation that makes readback need no row
                            // flip. Converting here mirrors the clip for
                            // exactly the reason an added row flip mirrors the
                            // image.
                            gl.enable(glow::SCISSOR_TEST);
                            gl.scissor(
                                clip.x as i32,
                                clip.y as i32,
                                clip.width as i32,
                                clip.height as i32,
                            );
                        }
                        None => gl.disable(glow::SCISSOR_TEST),
                    }
                    scissor = wanted;
                }
                // Bound for every draw, not only the ones that sample. A GLES
                // sampler left pointing at whatever unit a previous frame used
                // reads a texture that may since have been deleted, so the
                // placeholder is bound explicitly rather than by omission.
                let wanted = draw.material.texture_slot();
                if bound_texture != Some(wanted) {
                    let texture = match wanted {
                        Some(slot) => textures[slot as usize].texture,
                        None => placeholder,
                    };
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    if let Some(location) = &program.image {
                        gl.uniform_1_i32(Some(location), 0);
                    }
                    bound_texture = Some(wanted);
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
            // Both are global state that the next pass inherits, and a color
            // mask left off by a clip draw makes the following frame render
            // nothing at all.
            gl.disable(glow::STENCIL_TEST);
            gl.color_mask(true, true, true, true);
            gl.bind_vertex_array(None);
            detach_stencil(gl, render_fbo, stencil);
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
    /// Fill a texture from host memory, tightly packed and top row first.
    ///
    /// Stated in the same layout as [`Self::read_texture`], so a round trip
    /// through the pair is the identity on both backends.
    ///
    /// No row flip, for the same reason readback needs none: the shader
    /// translator negates Y for GLSL, which puts the image's top row at GL's
    /// zero. Flipping here would mean uploaded pixels and rendered ones
    /// disagreed about which way up they were, and only one of the two would
    /// look wrong.
    pub fn write_texture(&mut self, texture: &mut GlesTexture, pixels: &[u8]) -> Result<()> {
        let extent = texture.extent;
        let size = (extent.area() * 4) as usize;
        if pixels.len() != size {
            return Err(Error::Backend {
                backend: "gles",
                detail: format!("texture wants {size} bytes, {} supplied", pixels.len()),
            });
        }

        let gl = self.raw_gl();
        // SAFETY: a context is current and the source is sized for the region
        // being written.
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture.texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                extent.width as i32,
                extent.height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(pixels),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);

            let error = gl.get_error();
            if error != glow::NO_ERROR {
                return Err(Error::Backend {
                    backend: "gles",
                    detail: format!("upload produced GL error {error:#x}"),
                });
            }
        }
        Ok(())
    }

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
    // A blit is subject to the scissor test, so a clip left enabled from the
    // last draw would resolve only the part of the frame that draw could touch
    // and leave the rest of the target holding whatever it held before. Turning
    // it off here rather than at the call site covers every path that resolves,
    // including the early return for an empty batch.
    gl.disable(glow::SCISSOR_TEST);
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

/// Allocate a stencil renderbuffer and attach it to the bound framebuffer.
///
/// `STENCIL_INDEX8` is the only stencil format GLES 3.0 guarantees as
/// renderbuffer storage, and eight bits is also what the clip depth is checked
/// against, so the two limits agree by construction rather than by coincidence.
///
/// # Safety
///
/// A context must be current and a framebuffer must be bound.
unsafe fn attach_stencil(
    gl: &glow::Context,
    extent: Extent2D,
    samples: u32,
) -> Result<glow::Renderbuffer> {
    let renderbuffer = gl
        .create_renderbuffer()
        .map_err(|e| gl_err("create_renderbuffer", &e))?;
    gl.bind_renderbuffer(glow::RENDERBUFFER, Some(renderbuffer));
    // The sample count must match the color attachment beside it, which is
    // also what antialiases a clip edge: with a per-sample stencil, a boundary
    // crossing a pixel admits some of its samples and not others.
    if samples > 1 {
        gl.renderbuffer_storage_multisample(
            glow::RENDERBUFFER,
            samples as i32,
            glow::STENCIL_INDEX8,
            extent.width as i32,
            extent.height as i32,
        );
    } else {
        gl.renderbuffer_storage(
            glow::RENDERBUFFER,
            glow::STENCIL_INDEX8,
            extent.width as i32,
            extent.height as i32,
        );
    }
    gl.framebuffer_renderbuffer(
        glow::FRAMEBUFFER,
        glow::STENCIL_ATTACHMENT,
        glow::RENDERBUFFER,
        Some(renderbuffer),
    );
    gl.bind_renderbuffer(glow::RENDERBUFFER, None);

    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
    if status != glow::FRAMEBUFFER_COMPLETE {
        gl.framebuffer_renderbuffer(
            glow::FRAMEBUFFER,
            glow::STENCIL_ATTACHMENT,
            glow::RENDERBUFFER,
            None,
        );
        gl.delete_renderbuffer(renderbuffer);
        return Err(Error::Backend {
            backend: "gles",
            detail: format!("framebuffer incomplete with a stencil attachment: {status:#x}"),
        });
    }
    Ok(renderbuffer)
}

/// Detach and delete a stencil renderbuffer.
///
/// # Safety
///
/// A context must be current and `framebuffer` must belong to it.
unsafe fn detach_stencil(
    gl: &glow::Context,
    framebuffer: glow::Framebuffer,
    renderbuffer: Option<glow::Renderbuffer>,
) {
    let Some(renderbuffer) = renderbuffer else {
        return;
    };
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
    gl.framebuffer_renderbuffer(
        glow::FRAMEBUFFER,
        glow::STENCIL_ATTACHMENT,
        glow::RENDERBUFFER,
        None,
    );
    gl.delete_renderbuffer(renderbuffer);
}

/// Set the stencil test and write state one draw needs.
///
/// The color mask travels with it rather than being set separately, because the
/// two always change together: a clip draw contributes shape and no color, and
/// separating them leaves a state where one is set and the other is not.
fn apply_stencil(gl: &glow::Context, state: ClipState) {
    // SAFETY: a context is current.
    unsafe {
        gl.enable(glow::STENCIL_TEST);
        gl.stencil_func(glow::EQUAL, state.reference as i32, 0xff);
        let pass_op = match state.role {
            ClipRole::Content => glow::KEEP,
            // Saturating rather than wrapping, for the same reason the Vulkan
            // side clamps: at the top of the range a wrap lands on zero and
            // admits everything the clip excluded, where saturating merely
            // fails to narrow further. Depth is checked against the limit
            // before recording, so neither should arise.
            ClipRole::Narrow => glow::INCR,
            ClipRole::Widen => glow::DECR,
        };
        gl.stencil_op(glow::KEEP, glow::KEEP, pass_op);
        gl.stencil_mask(if state.role.writes_stencil() { 0xff } else { 0 });
        let color = state.role.writes_color();
        gl.color_mask(color, color, color, color);
    }
}

fn apply_blend(gl: &glow::Context, blend: BlendMode) {
    // SAFETY: a context is current.
    unsafe {
        if blend.is_plain_write() {
            // Writing the source outright needs no blend unit, which is worth
            // switching off rather than expressing as ONE and ZERO.
            gl.disable(glow::BLEND);
            return;
        }
        // Factors come from the shared table rather than being restated here,
        // so the two backends cannot disagree about what a mode means. It
        // assumes premultiplied colour, which is what the shader emits.
        //
        // An advanced mode has no factors at all, and submission has already
        // refused the batch by the time this runs, since this backend reports
        // no advanced-blend capability. Leaving blending disabled rather than
        // guessing keeps a future gap in that check visible as a missing blend
        // rather than as a plausible-looking wrong one.
        let Some(factors) = blend.factors() else {
            gl.disable(glow::BLEND);
            return;
        };
        let src = gl_blend_factor(factors.src);
        let dst = gl_blend_factor(factors.dst);
        gl.enable(glow::BLEND);
        // The same factors for alpha as for colour: with premultiplied colour
        // the alpha channel is not a special case, and giving it different
        // factors breaks compositing a layer onto something else.
        gl.blend_func_separate(src, dst, src, dst);
        gl.blend_equation(glow::FUNC_ADD);
    }
}

/// Translate a portable blend factor.
fn gl_blend_factor(factor: impeller_hal::BlendFactor) -> u32 {
    use impeller_hal::BlendFactor;
    match factor {
        BlendFactor::Zero => glow::ZERO,
        BlendFactor::One => glow::ONE,
        BlendFactor::SrcAlpha => glow::SRC_ALPHA,
        BlendFactor::OneMinusSrcAlpha => glow::ONE_MINUS_SRC_ALPHA,
        BlendFactor::DstAlpha => glow::DST_ALPHA,
        BlendFactor::OneMinusDstAlpha => glow::ONE_MINUS_DST_ALPHA,
        BlendFactor::DstColor => glow::DST_COLOR,
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

        // Naga names a combined sampler after the texture's group and binding
        // rather than after the WGSL variable, so this is the generated name
        // and not `image_texture`.
        let image = gl.get_uniform_location(program, "_group_0_binding_0_fs");

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
            image,
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

pub(crate) fn gl_err(what: &str, detail: &str) -> Error {
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
