//! Render passes, pipelines, and drawing triangles into a texture.

use crate::device::VulkanContext;
use crate::resource::{backend_err, vk_format, VulkanTexture};
use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;
use impeller_hal::{Error, Result};

/// Objects that depend only on the target's format, so they are built once per
/// format rather than per draw.
///
/// Pipeline creation costs milliseconds. Doing it per draw would put exactly
/// the compilation stall in the frame loop that compiling everything ahead of
/// time is meant to avoid.
pub(crate) struct SolidPipeline {
    pub(crate) render_pass: vk::RenderPass,
    pub(crate) layout: vk::PipelineLayout,
    pub(crate) pipeline: vk::Pipeline,
}

impl SolidPipeline {
    pub(crate) fn destroy(&self, device: &ash::Device) {
        // SAFETY: called from context teardown, after the queue has drained.
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_render_pass(self.render_pass, None);
        }
    }
}

impl VulkanContext {
    /// Draw indexed triangles into a texture, clearing it first.
    ///
    /// Positions are in normalized device coordinates following the **WGSL
    /// convention, where Y increases upward** — not Vulkan's native Y-down
    /// framebuffer convention.
    ///
    /// The difference is not incidental. Shaders are authored once in WGSL and
    /// translated per backend, and naga normalizes coordinate space so the
    /// same source behaves identically everywhere. Rendering against Vulkan's
    /// raw convention here would mean the same shader produced vertically
    /// mirrored output on Vulkan versus WebGPU, which is precisely what one
    /// source tree exists to prevent. Mapping user space, where Y typically
    /// runs downward, onto this belongs to the renderer's transform stack.
    ///
    /// Every draw clears. Load-preserving passes belong to the renderer, which
    /// owns pass grouping and knows when a target's contents matter.
    pub fn draw_indexed(
        &mut self,
        target: &mut VulkanTexture,
        vertices: &[[f32; 2]],
        indices: &[u32],
        clear: [f32; 4],
    ) -> Result<()> {
        if indices.len() % 3 != 0 {
            return Err(Error::Unsupported("index count is not a whole triangle"));
        }
        if let Some(&max) = indices.iter().max() {
            if max as usize >= vertices.len() {
                return Err(Error::Backend {
                    backend: "vulkan",
                    detail: format!(
                        "index {max} addresses past the {} vertices supplied",
                        vertices.len()
                    ),
                });
            }
        }

        let format = vk_format(target.format());
        self.ensure_solid_pipeline(format)?;
        let device = self.raw_device().clone();

        // Nothing to draw still clears, which is what a caller submitting an
        // empty scene expects.
        if indices.is_empty() {
            return self.clear_texture(target, clear);
        }

        let vertex_bytes: &[u8] = bytemuck_cast(vertices);
        let index_bytes: &[u8] = bytemuck_cast(indices);
        let vertex_buffer = self.upload(vertex_bytes, vk::BufferUsageFlags::VERTEX_BUFFER)?;
        let index_buffer = self.upload(index_bytes, vk::BufferUsageFlags::INDEX_BUFFER)?;

        let result = self.record_draw(
            &device,
            target,
            format,
            vertex_buffer.buffer,
            index_buffer.buffer,
            indices.len() as u32,
            clear,
        );

        self.release(vertex_buffer);
        self.release(index_buffer);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn record_draw(
        &mut self,
        device: &ash::Device,
        target: &mut VulkanTexture,
        format: vk::Format,
        vertex_buffer: vk::Buffer,
        index_buffer: vk::Buffer,
        index_count: u32,
        clear: [f32; 4],
    ) -> Result<()> {
        let cached = self.solid_pipeline(format).expect("ensured above");
        let (render_pass, pipeline, _) = (cached.render_pass, cached.pipeline, cached.layout);

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

        let extent = target.extent();
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

        let outcome = (|| -> Result<()> {
            let cmd = self.begin_one_shot()?;

            let clear_value = vk::ClearValue {
                color: vk::ClearColorValue { float32: clear },
            };
            let clear_values = [clear_value];
            let begin = vk::RenderPassBeginInfo::default()
                .render_pass(render_pass)
                .framebuffer(framebuffer)
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: vk::Extent2D {
                        width: extent.width,
                        height: extent.height,
                    },
                })
                .clear_values(&clear_values);

            let viewport = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: extent.width as f32,
                height: extent.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            let scissor = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: extent.width,
                    height: extent.height,
                },
            };

            unsafe {
                device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                device.cmd_set_viewport(cmd, 0, &[viewport]);
                device.cmd_set_scissor(cmd, 0, &[scissor]);
                device.cmd_bind_vertex_buffers(cmd, 0, &[vertex_buffer], &[0]);
                device.cmd_bind_index_buffer(cmd, index_buffer, 0, vk::IndexType::UINT32);
                device.cmd_draw_indexed(cmd, index_count, 1, 0, 0, 0);
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

    fn ensure_solid_pipeline(&mut self, format: vk::Format) -> Result<()> {
        if self.solid_pipeline(format).is_some() {
            return Ok(());
        }
        let device = self.raw_device().clone();
        let built = build_solid_pipeline(&device, format)?;
        self.insert_solid_pipeline(format, built);
        Ok(())
    }

    fn upload(&mut self, bytes: &[u8], usage: vk::BufferUsageFlags) -> Result<StagedBuffer> {
        let device = self.raw_device().clone();
        let info = vk::BufferCreateInfo::default()
            .size(bytes.len() as u64)
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
                // this path is for one-shot work.
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

fn build_solid_pipeline(device: &ash::Device, format: vk::Format) -> Result<SolidPipeline> {
    let attachment = vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        // CLEAR makes prior contents irrelevant, so declaring UNDEFINED here
        // saves a transition rather than losing anything.
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

    let color_ref = vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let color_refs = [color_ref];
    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_refs);

    let attachments = [attachment];
    let subpasses = [subpass];
    let rp_info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(&subpasses);
    let render_pass = unsafe { device.create_render_pass(&rp_info, None) }
        .map_err(|e| backend_err("create_render_pass", e))?;

    let module_info = vk::ShaderModuleCreateInfo::default().code(impeller_shaders::SOLID_SPV);
    let module = match unsafe { device.create_shader_module(&module_info, None) } {
        Ok(m) => m,
        Err(e) => {
            unsafe { device.destroy_render_pass(render_pass, None) };
            return Err(backend_err("create_shader_module", e));
        }
    };

    let layout_info = vk::PipelineLayoutCreateInfo::default();
    let layout = match unsafe { device.create_pipeline_layout(&layout_info, None) } {
        Ok(l) => l,
        Err(e) => {
            unsafe {
                device.destroy_shader_module(module, None);
                device.destroy_render_pass(render_pass, None);
            }
            return Err(backend_err("create_pipeline_layout", e));
        }
    };

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

    let binding = vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(std::mem::size_of::<[f32; 2]>() as u32)
        .input_rate(vk::VertexInputRate::VERTEX);
    let attribute = vk::VertexInputAttributeDescription::default()
        .location(0)
        .binding(0)
        .format(vk::Format::R32G32_SFLOAT)
        .offset(0);
    let bindings = [binding];
    let attributes = [attribute];
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
    let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)
        .blend_enable(false);
    let blend_attachments = [blend_attachment];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);

    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
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

    let pipelines = unsafe {
        device.create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
    };

    // The shader module is only needed during creation.
    unsafe { device.destroy_shader_module(module, None) };

    match pipelines {
        Ok(p) => Ok(SolidPipeline {
            render_pass,
            layout,
            pipeline: p[0],
        }),
        Err((_, e)) => {
            unsafe {
                device.destroy_pipeline_layout(layout, None);
                device.destroy_render_pass(render_pass, None);
            }
            Err(backend_err("create_graphics_pipelines", e))
        }
    }
}

/// Reinterpret a slice as bytes.
///
/// Both call sites pass slices of plain data with no padding and no invalid
/// bit patterns, and the result is only read.
fn bytemuck_cast<T>(slice: &[T]) -> &[u8] {
    // SAFETY: T is a plain-data type here ([f32; 2] or u32), every byte of it
    // is initialized, and the returned slice borrows the same memory for a
    // shorter-or-equal lifetime.
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}
