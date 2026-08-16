//! Render passes, pipelines, and drawing.
//!
//! Draws are accumulated into a [`Batch`] and submitted together: one geometry
//! upload, one render pass, one submission for the whole thing. A draw per
//! submission is fine for a test and hopeless for a frame loop, where the
//! fixed cost of beginning a pass and waiting on a fence dwarfs the drawing.

use crate::device::VulkanContext;
use crate::resource::{backend_err, transition, vk_format, VulkanTexture};
use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;
use impeller_hal::{Batch, BlendMode, Error, Result};
use std::collections::HashMap;

/// A render pass is distinguished by the attachment it targets and whether it
/// clears, because the load operation is baked into the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RenderPassKey {
    pub(crate) format: vk::Format,
    pub(crate) clears: bool,
}

/// A pipeline is distinguished by format and blend state.
///
/// Not by load operation: render passes with matching attachment formats and
/// sample counts are compatible, so one pipeline is usable with both the
/// clearing and the preserving pass. That compatibility is what lets a single
/// batch mix blend modes inside one pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PipelineKey {
    pub(crate) format: vk::Format,
    pub(crate) blend: BlendMode,
}

/// Cached pipeline objects, built on demand and held for the context's life.
///
/// Pipeline creation costs milliseconds, so building one per draw would put
/// exactly the compilation stall in the frame loop that compiling everything
/// ahead of time is meant to avoid.
#[derive(Default)]
pub(crate) struct PipelineCache {
    pub(crate) layout: Option<vk::PipelineLayout>,
    pub(crate) render_passes: HashMap<RenderPassKey, vk::RenderPass>,
    pub(crate) pipelines: HashMap<PipelineKey, vk::Pipeline>,
}

impl PipelineCache {
    pub(crate) fn render_pass(&self, key: RenderPassKey) -> Option<vk::RenderPass> {
        self.render_passes.get(&key).copied()
    }

    pub(crate) fn pipeline(&self, key: PipelineKey) -> Option<vk::Pipeline> {
        self.pipelines.get(&key).copied()
    }

    pub(crate) fn layout(&self) -> Option<vk::PipelineLayout> {
        self.layout
    }

    pub(crate) fn destroy(&mut self, device: &ash::Device) {
        // SAFETY: called from context teardown, after the queue has drained.
        unsafe {
            for pipeline in self.pipelines.values() {
                device.destroy_pipeline(*pipeline, None);
            }
            for pass in self.render_passes.values() {
                device.destroy_render_pass(*pass, None);
            }
            if let Some(layout) = self.layout {
                device.destroy_pipeline_layout(layout, None);
            }
        }
        self.pipelines.clear();
        self.render_passes.clear();
        self.layout = None;
    }
}

impl VulkanContext {
    /// Draw a single set of triangles. A convenience over [`Batch`].
    ///
    /// `color` is in linear space with **straight alpha**; it is premultiplied
    /// on the way to the target, which holds premultiplied color.
    pub fn draw_indexed(
        &mut self,
        target: &mut VulkanTexture,
        vertices: &[[f32; 2]],
        indices: &[u32],
        color: [f32; 4],
        blend: BlendMode,
        clear: Option<[f32; 4]>,
    ) -> Result<()> {
        let mut batch = Batch::new();
        batch.push(vertices, indices, color, blend)?;
        self.submit_batch(target, &batch, clear)
    }

    /// Record and submit a whole batch as one render pass.
    pub fn submit_batch(
        &mut self,
        target: &mut VulkanTexture,
        batch: &Batch,
        clear: Option<[f32; 4]>,
    ) -> Result<()> {
        let format = vk_format(target.format());

        // Nothing to draw still clears, which is what a caller submitting an
        // empty scene expects. With no clear either, there is nothing to do.
        if batch.is_empty() {
            return match clear {
                Some(c) => self.clear_texture(target, c),
                None => Ok(()),
            };
        }

        let pass_key = RenderPassKey {
            format,
            clears: clear.is_some(),
        };
        let render_pass = self.ensure_render_pass(pass_key)?;
        for draw in batch.draws() {
            self.ensure_pipeline(
                PipelineKey {
                    format,
                    blend: draw.blend,
                },
                render_pass,
            )?;
        }

        let device = self.raw_device().clone();
        let vertex_buffer = self.upload(
            cast_bytes(batch.vertices()),
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?;
        let index_buffer = self.upload(
            cast_bytes(batch.indices()),
            vk::BufferUsageFlags::INDEX_BUFFER,
        )?;

        let result = self.record_batch(
            &device,
            target,
            format,
            render_pass,
            batch,
            vertex_buffer.buffer,
            index_buffer.buffer,
            clear,
        );

        self.release(vertex_buffer);
        self.release(index_buffer);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn record_batch(
        &mut self,
        device: &ash::Device,
        target: &mut VulkanTexture,
        format: vk::Format,
        render_pass: vk::RenderPass,
        batch: &Batch,
        vertex_buffer: vk::Buffer,
        index_buffer: vk::Buffer,
        clear: Option<[f32; 4]>,
    ) -> Result<()> {
        let layout = self.pipeline_cache().layout().expect("ensured above");
        let extent = target.extent();

        let view_info = vk::ImageViewCreateInfo::default()
            .image(target.raw_image())
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        let view = unsafe { device.create_image_view(&view_info, None) }
            .map_err(|e| backend_err("create_image_view", e))?;

        let attachments = [view];
        let fb_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&attachments)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        let framebuffer = match unsafe { device.create_framebuffer(&fb_info, None) } {
            Ok(fb) => fb,
            Err(e) => {
                unsafe { device.destroy_image_view(view, None) };
                return Err(backend_err("create_framebuffer", e));
            }
        };

        let previous_layout = target.layout();
        let outcome = (|| -> Result<()> {
            let cmd = self.begin_one_shot()?;

            if clear.is_none() {
                // Loading requires the attachment already be in the layout the
                // render pass declares, and the texture may be sitting in
                // whatever a previous readback left it in.
                transition(
                    device,
                    cmd,
                    target.raw_image(),
                    previous_layout,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                );
            }

            let clear_values = [vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: clear.unwrap_or([0.0; 4]),
                },
            }];
            let area = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: extent.width,
                    height: extent.height,
                },
            };
            let begin = vk::RenderPassBeginInfo::default()
                .render_pass(render_pass)
                .framebuffer(framebuffer)
                .render_area(area)
                .clear_values(&clear_values);

            let viewport = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: extent.width as f32,
                height: extent.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };

            unsafe {
                device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
                device.cmd_set_viewport(cmd, 0, &[viewport]);
                device.cmd_set_scissor(cmd, 0, &[area]);
                device.cmd_bind_vertex_buffers(cmd, 0, &[vertex_buffer], &[0]);
                device.cmd_bind_index_buffer(cmd, index_buffer, 0, vk::IndexType::UINT32);

                // Bind only when the pipeline actually changes. Draws stay in
                // submission order, so consecutive draws sharing a blend mode
                // are common and rebinding each time is pure overhead.
                let mut bound: Option<BlendMode> = None;
                for draw in batch.draws() {
                    if bound != Some(draw.blend) {
                        let pipeline = self
                            .pipeline_cache()
                            .pipeline(PipelineKey {
                                format,
                                blend: draw.blend,
                            })
                            .expect("ensured above");
                        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                        bound = Some(draw.blend);
                    }
                    device.cmd_push_constants(
                        cmd,
                        layout,
                        vk::ShaderStageFlags::FRAGMENT,
                        0,
                        cast_bytes(&draw.color),
                    );
                    device.cmd_draw_indexed(cmd, draw.index_count, 1, draw.first_index, 0, 0);
                }

                device.cmd_end_render_pass(cmd);
            }

            self.submit_one_shot(cmd)
        })();

        // SAFETY: the submission above was waited on, so neither object is in
        // use regardless of the result.
        unsafe {
            device.destroy_framebuffer(framebuffer, None);
            device.destroy_image_view(view, None);
        }

        if outcome.is_ok() {
            // The render pass declares this as its final layout, so the next
            // operation must transition from here rather than from whatever
            // the texture held before.
            target.set_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        }
        outcome
    }

    fn ensure_render_pass(&mut self, key: RenderPassKey) -> Result<vk::RenderPass> {
        if let Some(pass) = self.pipeline_cache().render_pass(key) {
            return Ok(pass);
        }
        let device = self.raw_device().clone();
        let pass = build_render_pass(&device, key)?;
        self.pipeline_cache_mut().render_passes.insert(key, pass);
        Ok(pass)
    }

    fn ensure_pipeline(&mut self, key: PipelineKey, render_pass: vk::RenderPass) -> Result<()> {
        if self.pipeline_cache().pipeline(key).is_some() {
            return Ok(());
        }
        let device = self.raw_device().clone();
        let layout = match self.pipeline_cache().layout() {
            Some(l) => l,
            None => {
                let l = build_pipeline_layout(&device)?;
                self.pipeline_cache_mut().layout = Some(l);
                l
            }
        };
        let pipeline = build_pipeline(&device, key, render_pass, layout)?;
        self.pipeline_cache_mut().pipelines.insert(key, pipeline);
        Ok(())
    }

    fn upload(&mut self, bytes: &[u8], usage: vk::BufferUsageFlags) -> Result<StagedBuffer> {
        let device = self.raw_device().clone();
        let info = vk::BufferCreateInfo::default()
            .size(bytes.len().max(1) as u64)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.create_buffer(&info, None) }
            .map_err(|e| backend_err("create_buffer", e))?;

        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let mut allocation = self
            .allocator_mut()
            .allocate(&AllocationCreateDesc {
                name: "upload",
                requirements,
                // Host-visible so the write below needs no staging copy. Real
                // per-frame geometry goes through a ring allocator instead;
                // this path allocates per submission.
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| {
                unsafe { device.destroy_buffer(buffer, None) };
                Error::Backend {
                    backend: "vulkan",
                    detail: format!("upload allocation: {e}"),
                }
            })?;

        unsafe { device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()) }
            .map_err(|e| backend_err("bind_buffer_memory", e))?;

        let slice = allocation.mapped_slice_mut().ok_or(Error::Backend {
            backend: "vulkan",
            detail: "upload allocation was not host-visible".into(),
        })?;
        slice[..bytes.len()].copy_from_slice(bytes);

        Ok(StagedBuffer { buffer, allocation })
    }

    fn release(&mut self, staged: StagedBuffer) {
        let _ = self.allocator_mut().free(staged.allocation);
        // SAFETY: every submission using this buffer was waited on before the
        // call that made it returned.
        unsafe { self.raw_device().destroy_buffer(staged.buffer, None) };
    }
}

struct StagedBuffer {
    buffer: vk::Buffer,
    allocation: Allocation,
}

fn build_render_pass(device: &ash::Device, key: RenderPassKey) -> Result<vk::RenderPass> {
    // Clearing makes prior contents irrelevant, so the attachment can declare
    // UNDEFINED and skip a transition. Loading must declare the layout the
    // image is actually in, and the caller transitions it there first.
    let (load_op, initial_layout) = if key.clears {
        (vk::AttachmentLoadOp::CLEAR, vk::ImageLayout::UNDEFINED)
    } else {
        (
            vk::AttachmentLoadOp::LOAD,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        )
    };
    let attachments = [vk::AttachmentDescription::default()
        .format(key.format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(load_op)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(initial_layout)
        .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];

    let color_refs = [vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let subpasses = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_refs)];
    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(&subpasses);

    unsafe { device.create_render_pass(&info, None) }
        .map_err(|e| backend_err("create_render_pass", e))
}

fn build_pipeline_layout(device: &ash::Device) -> Result<vk::PipelineLayout> {
    // Paint travels as a push constant, so the layout declares the range even
    // though there are no descriptor sets. One layout serves every pipeline,
    // since they all take the same paint.
    let ranges = [vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
        .offset(0)
        .size(std::mem::size_of::<[f32; 4]>() as u32)];
    let info = vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&ranges);
    unsafe { device.create_pipeline_layout(&info, None) }
        .map_err(|e| backend_err("create_pipeline_layout", e))
}

fn build_pipeline(
    device: &ash::Device,
    key: PipelineKey,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    let module_info = vk::ShaderModuleCreateInfo::default().code(impeller_shaders::SOLID_SPV);
    let module = unsafe { device.create_shader_module(&module_info, None) }
        .map_err(|e| backend_err("create_shader_module", e))?;

    let vs_name = c"vs_main";
    let fs_name = c"fs_main";
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(module)
            .name(vs_name),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(module)
            .name(fs_name),
    ];

    let bindings = [vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(std::mem::size_of::<[f32; 2]>() as u32)
        .input_rate(vk::VertexInputRate::VERTEX)];
    let attributes = [vk::VertexInputAttributeDescription::default()
        .location(0)
        .binding(0)
        .format(vk::Format::R32G32_SFLOAT)
        .offset(0)];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&bindings)
        .vertex_attribute_descriptions(&attributes);

    let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let raster = vk::PipelineRasterizationStateCreateInfo::default()
        // No culling: 2D geometry arrives in whatever winding tessellation
        // produced, and discarding half of it by winding would drop triangles
        // that are perfectly visible.
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .polygon_mode(vk::PolygonMode::FILL)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);

    // Source color arrives premultiplied from the shader, so source-over is
    // ONE rather than SRC_ALPHA. Using SRC_ALPHA against a premultiplied
    // source would apply alpha twice and darken every translucent edge.
    let blend_attachments = [match key.blend {
        BlendMode::SrcOver => vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD),
        BlendMode::Src => vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(false),
    }];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);

    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&raster)
        .multisample_state(&multisample)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0);

    let created =
        unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) };

    // The shader module is only needed during creation.
    unsafe { device.destroy_shader_module(module, None) };

    match created {
        Ok(pipelines) => Ok(pipelines[0]),
        Err((_, e)) => Err(backend_err("create_graphics_pipelines", e)),
    }
}

/// Reinterpret a slice of plain data as bytes.
fn cast_bytes<T>(slice: &[T]) -> &[u8] {
    // SAFETY: T is a plain-data type here ([f32; 2], [f32; 4] or u32), every
    // byte of it is initialized, and the returned slice borrows the same memory
    // for a shorter-or-equal lifetime.
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}
