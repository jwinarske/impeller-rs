//! Vulkan backend via ash. First-class: the reference implementation, the
//! conformance oracle other backends are diffed against, and where new
//! features land first.
//!
//! Vulkan 1.1 is the floor; later features are used when present rather than
//! required. Features avoided for MoltenVK compatibility — geometry shaders,
//! tessellation shaders, sparse residency, multi-draw-indirect-with-count —
//! are none of them needed for 2D.
//!
//! # Status
//!
//! Device bring-up and capability detection only. The `Hal` and `HalContext`
//! traits are not implemented yet, because doing so means resource creation
//! and command recording; stubbing those to return errors would make the type
//! look usable while failing at the first draw.

pub mod device;
pub mod resource;
pub mod validation;

pub use device::{ContextConfig, DevicePreference, VulkanContext};
pub use resource::VulkanTexture;
pub use validation::{ValidationMessage, ValidationSeverity};
