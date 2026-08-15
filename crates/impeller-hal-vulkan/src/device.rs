//! Instance and device bring-up, and the capability detection everything
//! above the HAL branches on.
//!
//! Capability detection is the load-bearing part. The presentation layer
//! decides whether it can allocate its own scanout buffers or must import from
//! GBM, and whether the frame loop can stay explicit or needs a CPU wait
//! before commit, purely from what is reported here. Getting a bit wrong does
//! not fail loudly — it silently selects the slower path, or worse, selects
//! the fast path on a driver that cannot support it.

use ash::vk;
use impeller_hal::{Capabilities, DmaBufSupport, Error, Result, SampleCounts, SyncSupport};
use std::collections::HashSet;
use std::ffi::{c_char, CStr, CString};

/// Which physical device to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DevicePreference {
    /// Prefer discrete, then integrated, then anything else.
    #[default]
    Auto,
    /// Prefer a software rasterizer.
    ///
    /// The deterministic reference used for golden comparison: a CPU device
    /// produces the same pixels everywhere, which is what makes it usable as
    /// the oracle other backends are diffed against.
    Software,
    /// A specific index into the enumeration order.
    Index(usize),
}

/// Extensions the DRM presentation path depends on.
///
/// Absence is not an error — it selects a different path, and which one is
/// reported through [`Capabilities`] rather than inferred from the backend.
mod ext {
    pub const EXTERNAL_MEMORY_DMA_BUF: &str = "VK_EXT_external_memory_dma_buf";
    pub const EXTERNAL_MEMORY_FD: &str = "VK_KHR_external_memory_fd";
    pub const IMAGE_DRM_FORMAT_MODIFIER: &str = "VK_EXT_image_drm_format_modifier";
    pub const EXTERNAL_FENCE_FD: &str = "VK_KHR_external_fence_fd";
    pub const EXTERNAL_SEMAPHORE_FD: &str = "VK_KHR_external_semaphore_fd";
    pub const PHYSICAL_DEVICE_DRM: &str = "VK_EXT_physical_device_drm";
}

/// A Vulkan device, its queue, and what it can do.
pub struct VulkanContext {
    // Declaration order is destruction order: the device must outlive nothing
    // and the instance must outlive the device, so Drop tears down in reverse.
    device: ash::Device,
    physical_device: vk::PhysicalDevice,
    queue: vk::Queue,
    queue_family_index: u32,
    capabilities: Capabilities,
    enabled_extensions: HashSet<String>,
    instance: ash::Instance,
    _entry: ash::Entry,
}

impl VulkanContext {
    /// Create a context, selecting a physical device by preference.
    pub fn new(preference: DevicePreference) -> Result<Self> {
        // SAFETY: the loader is dlopened and the returned entry points are
        // used only for the lifetime of `entry`, which this struct owns.
        let entry = unsafe { ash::Entry::load() }.map_err(|e| Error::Backend {
            backend: "vulkan",
            detail: format!("loader: {e}"),
        })?;

        let app_name = CString::new("impeller-rs").unwrap();
        let app_info = vk::ApplicationInfo::default()
            .application_name(&app_name)
            .engine_name(&app_name)
            // 1.1 is the floor the project targets. Later features are used
            // when present rather than required.
            .api_version(vk::make_api_version(0, 1, 1, 0));

        let create_info = vk::InstanceCreateInfo::default().application_info(&app_info);
        let instance = unsafe { entry.create_instance(&create_info, None) }
            .map_err(|e| backend_err("create_instance", e))?;

        let physical_device = match select_physical_device(&instance, preference) {
            Ok(pd) => pd,
            Err(e) => {
                unsafe { instance.destroy_instance(None) };
                return Err(e);
            }
        };

        Self::finish(entry, instance, physical_device)
    }

    fn finish(
        entry: ash::Entry,
        instance: ash::Instance,
        physical_device: vk::PhysicalDevice,
    ) -> Result<Self> {
        let available = match device_extensions(&instance, physical_device) {
            Ok(set) => set,
            Err(e) => {
                unsafe { instance.destroy_instance(None) };
                return Err(e);
            }
        };

        // Request every DRM-relevant extension the device offers. Enabling one
        // that goes unused costs nothing; discovering later that it was not
        // enabled costs a device recreation.
        let wanted = [
            ext::EXTERNAL_MEMORY_DMA_BUF,
            ext::EXTERNAL_MEMORY_FD,
            ext::IMAGE_DRM_FORMAT_MODIFIER,
            ext::EXTERNAL_FENCE_FD,
            ext::EXTERNAL_SEMAPHORE_FD,
            ext::PHYSICAL_DEVICE_DRM,
        ];
        let enabled: HashSet<String> = wanted
            .iter()
            .filter(|n| available.contains(**n))
            .map(|n| n.to_string())
            .collect();

        let queue_family_index = match select_queue_family(&instance, physical_device) {
            Ok(i) => i,
            Err(e) => {
                unsafe { instance.destroy_instance(None) };
                return Err(e);
            }
        };

        let enabled_cstrings: Vec<CString> = enabled
            .iter()
            .map(|s| CString::new(s.as_str()).unwrap())
            .collect();
        let enabled_ptrs: Vec<*const c_char> =
            enabled_cstrings.iter().map(|s| s.as_ptr()).collect();

        let priorities = [1.0f32];
        let queue_info = vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&priorities);
        let queue_infos = [queue_info];
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&enabled_ptrs);

        let device = match unsafe { instance.create_device(physical_device, &device_info, None) } {
            Ok(d) => d,
            Err(e) => {
                unsafe { instance.destroy_instance(None) };
                return Err(backend_err("create_device", e));
            }
        };

        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        let capabilities = detect_capabilities(&instance, physical_device, &enabled);

        Ok(Self {
            device,
            physical_device,
            queue,
            queue_family_index,
            capabilities,
            enabled_extensions: enabled,
            instance,
            _entry: entry,
        })
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    pub fn queue_family_index(&self) -> u32 {
        self.queue_family_index
    }

    /// Whether a device extension was enabled at device creation.
    pub fn has_extension(&self, name: &str) -> bool {
        self.enabled_extensions.contains(name)
    }

    pub fn raw_device(&self) -> &ash::Device {
        &self.device
    }

    pub fn raw_physical_device(&self) -> vk::PhysicalDevice {
        self.physical_device
    }

    pub fn raw_queue(&self) -> vk::Queue {
        self.queue
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        // SAFETY: no command buffers are outstanding because none can be
        // created yet, and the device is destroyed before the instance that
        // created it.
        unsafe {
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

impl std::fmt::Debug for VulkanContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VulkanContext")
            .field("device", &self.capabilities.device_name)
            .field("driver", &self.capabilities.driver_name)
            .finish_non_exhaustive()
    }
}

fn backend_err(what: &str, e: vk::Result) -> Error {
    Error::Backend {
        backend: "vulkan",
        detail: format!("{what}: {e:?}"),
    }
}

fn device_extensions(instance: &ash::Instance, pd: vk::PhysicalDevice) -> Result<HashSet<String>> {
    let props = unsafe { instance.enumerate_device_extension_properties(pd) }
        .map_err(|e| backend_err("enumerate_device_extension_properties", e))?;
    Ok(props
        .iter()
        .filter_map(|p| {
            // SAFETY: the driver guarantees a NUL-terminated name.
            let raw = unsafe { CStr::from_ptr(p.extension_name.as_ptr()) };
            raw.to_str().ok().map(|s| s.to_owned())
        })
        .collect())
}

fn select_physical_device(
    instance: &ash::Instance,
    preference: DevicePreference,
) -> Result<vk::PhysicalDevice> {
    let devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|e| backend_err("enumerate_physical_devices", e))?;
    if devices.is_empty() {
        return Err(Error::Backend {
            backend: "vulkan",
            detail: "no physical devices".into(),
        });
    }

    match preference {
        DevicePreference::Index(i) => devices.get(i).copied().ok_or(Error::Backend {
            backend: "vulkan",
            detail: format!("device index {i} out of range ({} present)", devices.len()),
        }),
        DevicePreference::Auto => Ok(*devices
            .iter()
            .max_by_key(|pd| {
                let props = unsafe { instance.get_physical_device_properties(**pd) };
                match props.device_type {
                    vk::PhysicalDeviceType::DISCRETE_GPU => 3,
                    vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
                    vk::PhysicalDeviceType::VIRTUAL_GPU => 1,
                    _ => 0,
                }
            })
            .expect("non-empty")),
        DevicePreference::Software => devices
            .iter()
            .find(|pd| {
                let props = unsafe { instance.get_physical_device_properties(**pd) };
                props.device_type == vk::PhysicalDeviceType::CPU
            })
            .copied()
            .ok_or(Error::Unsupported("no software rasterizer present")),
    }
}

fn select_queue_family(instance: &ash::Instance, pd: vk::PhysicalDevice) -> Result<u32> {
    let families = unsafe { instance.get_physical_device_queue_family_properties(pd) };
    families
        .iter()
        .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .map(|i| i as u32)
        .ok_or(Error::Unsupported("no graphics queue family"))
}

fn detect_capabilities(
    instance: &ash::Instance,
    pd: vk::PhysicalDevice,
    enabled: &HashSet<String>,
) -> Capabilities {
    let props = unsafe { instance.get_physical_device_properties(pd) };
    let limits = props.limits;

    // Sample counts a target can use are the intersection of what color and
    // depth attachments support. Reporting the color count alone would promise
    // a configuration that fails once depth is attached.
    let sample_mask =
        limits.framebuffer_color_sample_counts & limits.framebuffer_depth_sample_counts;

    // dma-buf export needs both the fd machinery and the dma-buf handle type;
    // either alone is useless. Negotiating a scanout-capable layout
    // additionally needs explicit modifiers.
    let fd = enabled.contains(ext::EXTERNAL_MEMORY_FD);
    let dma_buf = enabled.contains(ext::EXTERNAL_MEMORY_DMA_BUF);
    let modifiers = enabled.contains(ext::IMAGE_DRM_FORMAT_MODIFIER);

    Capabilities {
        max_texture_size: limits.max_image_dimension2_d,
        sample_counts: SampleCounts::from_mask(sample_mask.as_raw()),
        dma_buf: DmaBufSupport {
            import: fd && dma_buf,
            export: fd && dma_buf,
            modifiers,
        },
        sync: SyncSupport {
            export_sync_file: enabled.contains(ext::EXTERNAL_FENCE_FD),
            import_sync_file: enabled.contains(ext::EXTERNAL_FENCE_FD),
        },
        render_formats: Vec::new(),
        device_name: device_name(&props),
        driver_name: format!("vulkan {}", api_version_string(props.api_version)),
    }
}

fn device_name(props: &vk::PhysicalDeviceProperties) -> String {
    // SAFETY: the driver guarantees a NUL-terminated name.
    unsafe { CStr::from_ptr(props.device_name.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn api_version_string(v: u32) -> String {
    format!(
        "{}.{}.{}",
        vk::api_version_major(v),
        vk::api_version_minor(v),
        vk::api_version_patch(v)
    )
}
