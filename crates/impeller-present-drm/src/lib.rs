//! DRM/KMS direct-scanout presentation: rendering straight to a display with no
//! compositor present, the configuration embedded, automotive, kiosk, and
//! industrial products actually ship.
//!
//! DRM is a presentation target, not a third rendering backend. Both rendering
//! backends reach it: Vulkan exports VkImages as dma-bufs, and GLES renders
//! into a gbm_surface.
//!
//! All KMS logic belongs to drm-rs -- connector, CRTC and plane discovery, mode
//! selection, atomic commit construction, page-flip events, hotplug, and
//! framebuffer import. The dependency direction is one-way and this crate does
//! not reimplement any of it. What lives here is buffer allocation, image
//! import and export, frame pacing against flip completion, and fence
//! plumbing.
