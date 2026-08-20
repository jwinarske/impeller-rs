//! Texture bindings for the image paint.
//!
//! # Why every draw binds a texture
//!
//! The shader is one program serving every material kind, so it declares the
//! texture and sampler whether a given draw samples them or not. Vulkan wants
//! every descriptor a pipeline statically uses to be bound, and "statically
//! uses" does not care that the branch reading it is unreachable for a solid
//! fill. The alternatives are a pipeline variant per material kind, which
//! multiplies the cache by four to avoid one binding, or leaving the descriptor
//! unbound, which is invalid. So a draw that samples nothing binds a one-pixel
//! placeholder instead.
//!
//! # Why one sampler rather than one per tile mode
//!
//! Address modes are a property of the paint, and baking them into samplers
//! would mean a sampler per combination and a descriptor set per draw that used
//! a different one. The shader does the wrapping arithmetic itself and the
//! sampler stays fixed at clamp-to-edge, which also keeps repeat and decal from
//! depending on filtering behavior at the seam.

use crate::device::VulkanContext;
use crate::resource::{backend_err, VulkanTexture};
use ash::vk;
use impeller_hal::{Batch, Error, Result};

/// The descriptor set layout every pipeline is built against.
///
/// Two bindings rather than one combined image sampler, because that is what
/// the shader declares: WGSL separates a `texture_2d` from a `sampler`, and
/// naga carries the separation into SPIR-V.
pub fn create_descriptor_layout(device: &ash::Device) -> Result<vk::DescriptorSetLayout> {
    let bindings = [
        vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        vk::DescriptorSetLayoutBinding::default()
            .binding(1)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT),
    ];
    let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    unsafe { device.create_descriptor_set_layout(&info, None) }
        .map_err(|e| backend_err("create_descriptor_set_layout", e))
}

/// The one sampler every image draw uses.
pub fn create_sampler(device: &ash::Device) -> Result<vk::Sampler> {
    let info = vk::SamplerCreateInfo::default()
        .mag_filter(vk::Filter::LINEAR)
        .min_filter(vk::Filter::LINEAR)
        // Between levels as well as within one, which is what trilinear means
        // and what `FilterQuality.medium` asks for. It changes nothing for the
        // draws that do not want it: the shader names the level it reads, and
        // every path but the mipmapped one names zero.
        .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
        // Without this the maximum level of detail defaults to zero and the
        // sampler clamps every read back to the largest level, which looks
        // exactly like a chain that was never generated.
        .max_lod(vk::LOD_CLAMP_NONE)
        // Clamped in the sampler and wrapped in the shader; see the module
        // note. Clamping here means a repeat's seam interpolates between the
        // texels the shader's own coordinate names, rather than across the
        // image's opposite edge.
        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
    unsafe { device.create_sampler(&info, None) }.map_err(|e| backend_err("create_sampler", e))
}

/// Descriptor sets and image views for one submission.
///
/// Built per submission and torn down with it, matching the multisample and
/// stencil buffers: the alternative is a cache keyed by texture identity, whose
/// invalidation rule has to answer what happens when a caller destroys a
/// texture a set still points at.
pub struct Bindings {
    /// `None` where the batch samples nothing and only the context's
    /// placeholder set is needed, which costs no allocation at all.
    pool: Option<vk::DescriptorPool>,
    views: Vec<vk::ImageView>,
    /// One set per texture slot.
    sets: Vec<vk::DescriptorSet>,
    /// Owned by the context and outliving any one submission, which is what
    /// lets a deferred submission use it: its descriptor pool would otherwise
    /// have to survive until a fence the caller retires whenever it likes.
    placeholder: vk::DescriptorSet,
}

impl Bindings {
    /// Bindings for a batch that samples nothing.
    pub fn placeholder_only(placeholder: vk::DescriptorSet) -> Self {
        Self {
            pool: None,
            views: Vec::new(),
            sets: Vec::new(),
            placeholder,
        }
    }

    /// The set a draw's material selects.
    ///
    /// A material naming no texture gets the placeholder, which is why this
    /// cannot fail: the table was checked to cover every slot when it was
    /// built.
    pub fn set_for(&self, slot: Option<u32>) -> vk::DescriptorSet {
        match slot {
            Some(slot) => self.sets[slot as usize],
            None => self.placeholder,
        }
    }

    pub fn destroy(self, ctx: &VulkanContext) {
        let device = ctx.raw_device();
        // SAFETY: the submission that used these was waited on, and destroying
        // the pool frees every set allocated from it.
        unsafe {
            for view in self.views {
                device.destroy_image_view(view, None);
            }
            if let Some(pool) = self.pool {
                device.destroy_descriptor_pool(pool, None);
            }
        }
    }
}

/// The pool, view and set for the placeholder texture.
///
/// Its own pool rather than a set carved from a submission's, so that its
/// lifetime is the context's and nothing has to reason about when the last
/// submission using it finished.
pub fn create_placeholder_binding(
    device: &ash::Device,
    image: vk::Image,
    layout: vk::DescriptorSetLayout,
    sampler: vk::Sampler,
) -> Result<(vk::DescriptorPool, vk::ImageView, vk::DescriptorSet)> {
    let sizes = [
        vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(1),
        vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::SAMPLER)
            .descriptor_count(1),
    ];
    let pool_info = vk::DescriptorPoolCreateInfo::default()
        .max_sets(1)
        .pool_sizes(&sizes);
    let pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
        .map_err(|e| backend_err("create_descriptor_pool", e))?;

    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(vk::Format::R8G8B8A8_UNORM)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1),
        );
    let view = match unsafe { device.create_image_view(&view_info, None) } {
        Ok(view) => view,
        Err(e) => {
            unsafe { device.destroy_descriptor_pool(pool, None) };
            return Err(backend_err("create_image_view", e));
        }
    };

    let layouts = [layout];
    let alloc = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(pool)
        .set_layouts(&layouts);
    let set = match unsafe { device.allocate_descriptor_sets(&alloc) } {
        Ok(sets) => sets[0],
        Err(e) => {
            unsafe {
                device.destroy_image_view(view, None);
                device.destroy_descriptor_pool(pool, None);
            }
            return Err(backend_err("allocate_descriptor_sets", e));
        }
    };

    let image_info = [vk::DescriptorImageInfo::default()
        .image_view(view)
        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
    let sampler_info = [vk::DescriptorImageInfo::default().sampler(sampler)];
    let writes = [
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&image_info),
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .image_info(&sampler_info),
    ];
    // SAFETY: the set comes from the pool above and nothing reads it yet.
    unsafe { device.update_descriptor_sets(&writes, &[]) };
    Ok((pool, view, set))
}

/// Build the bindings a batch needs from the table it was given.
pub fn build(
    ctx: &mut VulkanContext,
    batch: &Batch,
    textures: &[&VulkanTexture],
) -> Result<Bindings> {
    // Checked before anything is allocated, so a batch naming a slot nobody
    // supplied fails without leaving objects behind.
    for slot in batch.texture_slots() {
        if slot as usize >= textures.len() {
            return Err(Error::Backend {
                backend: "vulkan",
                detail: format!(
                    "a draw samples texture slot {slot}, but only {} were supplied",
                    textures.len()
                ),
            });
        }
    }

    let placeholder = ctx.placeholder_set()?;
    if textures.is_empty() {
        return Ok(Bindings::placeholder_only(placeholder));
    }

    // Built before the borrow below, since each of these creates its object on
    // first use and so needs the context mutably.
    let layout = ctx.descriptor_layout()?;
    let sampler = ctx.sampler()?;
    let device = ctx.raw_device().clone();

    let count = textures.len() as u32;
    let sizes = [
        vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(count),
        vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::SAMPLER)
            .descriptor_count(count),
    ];
    let pool_info = vk::DescriptorPoolCreateInfo::default()
        .max_sets(count)
        .pool_sizes(&sizes);
    let pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
        .map_err(|e| backend_err("create_descriptor_pool", e))?;

    let mut bindings = Bindings {
        pool: Some(pool),
        views: Vec::new(),
        sets: Vec::new(),
        placeholder,
    };

    let layouts = vec![layout; count as usize];
    let alloc = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(pool)
        .set_layouts(&layouts);
    bindings.sets = match unsafe { device.allocate_descriptor_sets(&alloc) } {
        Ok(sets) => sets,
        Err(e) => {
            bindings.destroy(ctx);
            return Err(backend_err("allocate_descriptor_sets", e));
        }
    };

    let images: Vec<(vk::Image, vk::Format)> = textures
        .iter()
        .map(|t| (t.raw_image(), crate::resource::vk_format(t.format())))
        .collect();
    for (index, (image, format)) in images.iter().enumerate() {
        let view_info = vk::ImageViewCreateInfo::default()
            .image(*image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(*format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    // Whatever the image has. A view over one level of a
                    // mipmapped image is a view a sampler cannot minify
                    // through, and the failure is silent -- the chain is there,
                    // the sampler is willing, and every read still lands on the
                    // largest level.
                    .level_count(vk::REMAINING_MIP_LEVELS)
                    .layer_count(1),
            );
        let view = match unsafe { device.create_image_view(&view_info, None) } {
            Ok(view) => view,
            Err(e) => {
                bindings.destroy(ctx);
                return Err(backend_err("create_image_view", e));
            }
        };
        bindings.views.push(view);

        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let sampler_info = [vk::DescriptorImageInfo::default().sampler(sampler)];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(bindings.sets[index])
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&image_info),
            vk::WriteDescriptorSet::default()
                .dst_set(bindings.sets[index])
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(&sampler_info),
        ];
        // SAFETY: the sets come from the pool above, the view outlives the
        // submission, and nothing is reading these yet.
        unsafe { device.update_descriptor_sets(&writes, &[]) };
    }

    Ok(bindings)
}

/// Move every texture a batch samples into the layout a shader reads from.
///
/// Sampled textures are usually render targets that a previous pass left in a
/// color-attachment layout, or fresh uploads sitting in a transfer layout.
/// Reading either without a transition is undefined, and on a tiled or
/// compressed-framebuffer device it is undefined in the way that produces a
/// plausible but wrong image rather than nothing at all.
pub fn transition_for_sampling(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    textures: &[&VulkanTexture],
) {
    for texture in textures {
        crate::resource::transition(
            device,
            cmd,
            texture.raw_image(),
            texture.layout(),
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        texture.set_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
    }
}
