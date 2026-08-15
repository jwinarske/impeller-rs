//! Vulkan backend via ash and gpu-allocator. First-class: the reference
//! implementation, the conformance oracle other backends are diffed against,
//! and where new features land first.
//!
//! Vulkan 1.1 floor; 1.3 dynamic rendering, timeline semaphores, and sync2 used
//! opportunistically. Avoids geometry shaders, tessellation shaders, sparse
//! residency, and multi-draw-indirect-with-count for MoltenVK compatibility --
//! none are needed for 2D.
