//! Images, their memory, and getting pixels back to the CPU.
//!
//! Readback exists for the test apparatus rather than for the frame loop. The
//! entire golden and conformance corpus renders to an offscreen target and
//! compares the result, so this path is what turns "the device reported a
//! capability" into "the device produced these pixels".

use crate::device::VulkanContext;
use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;
use impeller_hal::{Error, Extent2D, PixelFormat, Result, TextureDescriptor, TextureUsage};

/// An image, its backing memory, and the state needed to transition it.
///
/// Ownership is explicit: a texture must be handed back to
/// [`VulkanContext::destroy_texture`]. Automatic retirement belongs to the
/// fence waiter, which tracks GPU completion and cannot be built until there
/// is asynchronous work to track.
pub struct VulkanTexture {
    pub(crate) image: vk::Image,
    pub(crate) memory: TextureMemory,
    pub(crate) extent: Extent2D,
    pub(crate) format: PixelFormat,
    /// The layout the image is currently in, so the next operation knows what
    /// to transition from.
    /// The layout the image is currently in.
    ///
    /// Behind a cell because this tracks device-side state, not anything Rust
    /// aliasing rules are about: a texture being *sampled* is borrowed shared,
    /// and getting it into a readable layout is a transition the recorder has
    /// to make and record. Requiring a unique borrow for that would mean a
    /// batch could sample only one texture at a time.
    pub(crate) layout: std::cell::Cell<vk::ImageLayout>,
    pub(crate) usage: TextureUsage,
}

/// Where a texture's memory came from.
///
/// The distinction matters only at export: a dma-buf hands over a whole
/// allocation, so an image sharing one with other resources cannot be exported
/// without exporting them too.
pub(crate) enum TextureMemory {
    /// Suballocated from a pool. The common case, and not exportable.
    Pooled(Allocation),
    /// A whole allocation of its own, which is what export requires.
    Dedicated(vk::DeviceMemory),
}

impl VulkanTexture {
    pub fn extent(&self) -> Extent2D {
        self.extent
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn raw_image(&self) -> vk::Image {
        self.image
    }

    pub(crate) fn layout(&self) -> vk::ImageLayout {
        self.layout.get()
    }

    /// Record a layout change made by an operation outside this module.
    pub(crate) fn set_layout(&self, layout: vk::ImageLayout) {
        self.layout.set(layout);
    }

    /// Bytes a tightly packed readback of this texture occupies.
    pub fn byte_size(&self) -> u64 {
        self.extent.area() * self.format.bytes_per_pixel() as u64
    }
}

/// Map a portable format onto Vulkan's enumeration.
pub(crate) fn vk_format(format: PixelFormat) -> vk::Format {
    match format {
        PixelFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        PixelFormat::Rgba8UnormSrgb => vk::Format::R8G8B8A8_SRGB,
        PixelFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        PixelFormat::Bgra8UnormSrgb => vk::Format::B8G8R8A8_SRGB,
        PixelFormat::Rgb10A2Unorm => vk::Format::A2B10G10R10_UNORM_PACK32,
        PixelFormat::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
    }
}

impl VulkanContext {
    /// Allocate a texture.
    ///
    /// External images are not handled yet: importing a dma-buf needs the
    /// external-memory plumbing, and accepting the descriptor while ignoring
    /// its external half would hand back a blank image that silently is not
    /// the caller's buffer.
    pub fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<VulkanTexture> {
        if desc.is_external() {
            return Err(Error::Unsupported("external image import"));
        }
        if !self.capabilities().can_allocate(desc.extent) {
            return Err(Error::LimitExceeded {
                what: "texture dimension",
                requested: desc.extent.width.max(desc.extent.height) as u64,
                limit: self.capabilities().max_texture_size as u64,
            });
        }
        if desc.extent.is_empty() {
            return Err(Error::Unsupported("zero-sized texture"));
        }

        let mut usage = vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST;
        if desc.usage.sampled {
            usage |= vk::ImageUsageFlags::SAMPLED;
        }
        if desc.usage.render_target {
            usage |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
        }

        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format(desc.format))
            .extent(vk::Extent3D {
                width: desc.extent.width,
                height: desc.extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(sample_count_flags(desc.sample_count))
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);

        let device = self.raw_device().clone();
        let image = unsafe { device.create_image(&info, None) }
            .map_err(|e| backend_err("create_image", e))?;

        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let allocation = self
            .allocator_mut()
            .allocate(&AllocationCreateDesc {
                name: "texture",
                requirements,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| {
                unsafe { device.destroy_image(image, None) };
                Error::OutOfMemory {
                    what: "texture memory",
                }
                .with_detail(e)
            })?;

        unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset()) }
            .map_err(|e| backend_err("bind_image_memory", e))?;

        Ok(VulkanTexture {
            image,
            memory: TextureMemory::Pooled(allocation),
            extent: desc.extent,
            format: desc.format,
            layout: std::cell::Cell::new(vk::ImageLayout::UNDEFINED),
            usage: desc.usage,
        })
    }

    /// Release a texture and its memory.
    pub fn destroy_texture(&mut self, texture: VulkanTexture) {
        // Freeing has to match how the memory was obtained, or the pool is told
        // about an allocation it never made.
        match texture.memory {
            TextureMemory::Pooled(allocation) => {
                let _ = self.allocator_mut().free(allocation);
            }
            TextureMemory::Dedicated(memory) => {
                // SAFETY: nothing else holds this allocation, and every
                // submission using the image was waited on.
                unsafe { self.raw_device().free_memory(memory, None) };
            }
        }
        // SAFETY: the caller has given up the texture, and every submission
        // that used it was waited on before returning from the call that made
        // it.
        unsafe { self.raw_device().destroy_image(texture.image, None) };
    }

    /// Clear a texture to a solid color.
    ///
    /// Records, submits, and waits. Batching belongs to the renderer, which
    /// does not exist yet; doing it here would be an abstraction guessing at
    /// its caller.
    pub fn clear_texture(&mut self, texture: &mut VulkanTexture, color: [f32; 4]) -> Result<()> {
        let device = self.raw_device().clone();
        let cmd = self.begin_one_shot()?;

        transition(
            &device,
            cmd,
            texture.image,
            texture.layout.get(),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );

        let clear = vk::ClearColorValue { float32: color };
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        unsafe {
            device.cmd_clear_color_image(
                cmd,
                texture.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &clear,
                &[range],
            );
        }
        texture.layout.set(vk::ImageLayout::TRANSFER_DST_OPTIMAL);

        self.submit_one_shot(cmd)
    }

    /// Copy a texture back to host memory, tightly packed.
    /// Fill a texture from host memory, tightly packed and top row first.
    ///
    /// The mirror of [`Self::read_texture`], and stated in the same layout, so
    /// a round trip through the pair is the identity. That is what makes it
    /// testable without a decoder: write known bytes, read them back, compare.
    ///
    /// Image *decoding* is out of scope for this project, but getting decoded
    /// pixels onto the device is not — without this an image shader would have
    /// nothing to sample but what the renderer itself drew.
    pub fn write_texture(&mut self, texture: &mut VulkanTexture, pixels: &[u8]) -> Result<()> {
        let size = texture.byte_size();
        if pixels.len() as u64 != size {
            return Err(Error::Backend {
                backend: "vulkan",
                detail: format!("texture wants {size} bytes, {} supplied", pixels.len()),
            });
        }
        let device = self.raw_device().clone();

        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.create_buffer(&buffer_info, None) }
            .map_err(|e| backend_err("create_buffer", e))?;
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let mut allocation = self
            .allocator_mut()
            .allocate(&AllocationCreateDesc {
                name: "upload",
                requirements,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| {
                unsafe { device.destroy_buffer(buffer, None) };
                Error::OutOfMemory {
                    what: "upload buffer",
                }
                .with_detail(e)
            })?;
        if let Err(e) =
            unsafe { device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()) }
        {
            let _ = self.allocator_mut().free(allocation);
            unsafe { device.destroy_buffer(buffer, None) };
            return Err(backend_err("bind_buffer_memory", e));
        }

        let staged = match allocation.mapped_slice_mut() {
            Some(slice) => {
                slice[..pixels.len()].copy_from_slice(pixels);
                Ok(())
            }
            None => Err(Error::Backend {
                backend: "vulkan",
                detail: "upload allocation was not host-visible".into(),
            }),
        };

        let outcome = staged.and_then(|()| {
            let cmd = self.begin_one_shot()?;
            transition(
                &device,
                cmd,
                texture.image,
                texture.layout.get(),
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            );
            let region = vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: texture.extent.width,
                    height: texture.extent.height,
                    depth: 1,
                });
            // SAFETY: the command buffer is recording, the buffer holds the
            // staged pixels, and the image is in the layout named here.
            unsafe {
                device.cmd_copy_buffer_to_image(
                    cmd,
                    buffer,
                    texture.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
            }
            texture.layout.set(vk::ImageLayout::TRANSFER_DST_OPTIMAL);
            self.submit_one_shot(cmd)
        });

        let _ = self.allocator_mut().free(allocation);
        // SAFETY: the submission above was waited on, so the copy has finished
        // reading from this buffer.
        unsafe { device.destroy_buffer(buffer, None) };
        outcome
    }

    pub fn read_texture(&mut self, texture: &mut VulkanTexture) -> Result<Vec<u8>> {
        let size = texture.byte_size();
        let device = self.raw_device().clone();

        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.create_buffer(&buffer_info, None) }
            .map_err(|e| backend_err("create_buffer", e))?;
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let allocation = self
            .allocator_mut()
            .allocate(&AllocationCreateDesc {
                name: "readback",
                requirements,
                location: MemoryLocation::GpuToCpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| {
                unsafe { device.destroy_buffer(buffer, None) };
                Error::OutOfMemory {
                    what: "readback buffer",
                }
                .with_detail(e)
            })?;
        unsafe { device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()) }
            .map_err(|e| backend_err("bind_buffer_memory", e))?;

        let cmd = self.begin_one_shot()?;
        transition(
            &device,
            cmd,
            texture.image,
            texture.layout.get(),
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        );
        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width: texture.extent.width,
                height: texture.extent.height,
                depth: 1,
            });
        unsafe {
            device.cmd_copy_image_to_buffer(
                cmd,
                texture.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[region],
            );
        }
        texture.layout.set(vk::ImageLayout::TRANSFER_SRC_OPTIMAL);

        // The buffer and its allocation are released on every path below, so
        // the submission result is folded in rather than returned early.
        let pixels = self.submit_one_shot(cmd).and_then(|()| {
            // The submission was waited on, so the copy has completed and the
            // mapping is stable for the read.
            allocation
                .mapped_slice()
                .map(|s| s[..size as usize].to_vec())
                .ok_or(Error::Backend {
                    backend: "vulkan",
                    detail: "readback allocation was not host-visible".into(),
                })
        });

        let _ = self.allocator_mut().free(allocation);
        unsafe { device.destroy_buffer(buffer, None) };
        pixels
    }
}

/// Insert a full barrier around a layout change.
///
/// Deliberately heavy-handed: ALL_COMMANDS on both sides with full access
/// masks. These are one-shot setup and readback operations, not frame-loop
/// work, so precision here buys nothing and getting it wrong costs
/// hard-to-reproduce corruption.
pub(crate) fn transition(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    from: vk::ImageLayout,
    to: vk::ImageLayout,
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .old_layout(from)
        .new_layout(to)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1),
        )
        .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
        .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE);

    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}

fn sample_count_flags(count: u32) -> vk::SampleCountFlags {
    match count {
        2 => vk::SampleCountFlags::TYPE_2,
        4 => vk::SampleCountFlags::TYPE_4,
        8 => vk::SampleCountFlags::TYPE_8,
        16 => vk::SampleCountFlags::TYPE_16,
        _ => vk::SampleCountFlags::TYPE_1,
    }
}

pub(crate) fn backend_err(what: &str, e: vk::Result) -> Error {
    Error::Backend {
        backend: "vulkan",
        detail: format!("{what}: {e:?}"),
    }
}

/// Attach backend detail to an allocator failure without losing the category.
pub(crate) trait WithDetail {
    fn with_detail<E: std::fmt::Display>(self, e: E) -> Error;
}

impl WithDetail for Error {
    fn with_detail<E: std::fmt::Display>(self, e: E) -> Error {
        Error::Backend {
            backend: "vulkan",
            detail: format!("{self}: {e}"),
        }
    }
}
