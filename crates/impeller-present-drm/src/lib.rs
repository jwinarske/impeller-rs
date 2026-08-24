//! DRM/KMS direct scanout: rendering straight to a display with no compositor.
//!
//! This is the configuration embedded, automotive, kiosk, and industrial
//! products actually ship, and the one most renderers leave as an exercise. It
//! is a presentation target, not a rendering backend: the renderer draws through
//! a HAL as always, and this decides how the result reaches a panel.
//!
//! Everything to do with KMS belongs to drm-rs and is consumed through
//! [`output::ScanoutOutput`], which states exactly what this crate requires.
//! Expressing it as a trait lets the two projects be sequenced against each
//! other, and makes the parts most worth testing — ring accounting and fence
//! plumbing — testable without a display attached.
//!
//! # It assumes the renderer's device can export what the display can import
//!
//! The whole path is: render into a buffer, export it as a dma-buf, hand the fd
//! to the display controller, scan it out. That works where one device does
//! both, and where two devices can share memory. It is not universal, and the
//! first real board it was run on is one where it does not hold.
//!
//! On a Raspberry Pi 4 the render device and the display controller are
//! separate DRM devices — `v3d` on `card0`, `vc4` on `card1` — and importing a
//! Vulkan-exported buffer into the display controller fails outright:
//!
//! ```text
//! driving /dev/dri/card1 at 1280x1440
//! agreed on Fourcc(875713089) with Modifier(0)
//! prime_fd_to_buffer: Invalid argument (os error 22)
//! ```
//!
//! Everything up to that point works, which is what makes the failure precise
//! rather than general: DRM master is taken, the connector and mode are read,
//! and format negotiation agrees on `XR24` with the linear modifier — a format
//! and layout the display controller itself advertised. What it will not do is
//! take the memory. Nothing is logged by the kernel and CMA is not exhausted,
//! so this is the allocation being unsuitable rather than unavailable.
//!
//! The direction is the thing to reconsider, and it is a design question rather
//! than a defect to patch. A display controller with no IOMMU can only scan out
//! memory it can address, which for `vc4` means contiguous; a render device with
//! an MMU has no reason to allocate that way and `v3d` does not. Boards like
//! this want the buffer allocated on the *display* side and imported into the
//! renderer — the opposite of what happens here — which is a different shape for
//! [`target::DrmScanoutTarget`] rather than a fix to its import call.
//!
//! Recorded rather than worked around because nothing in this repository can
//! reach it: the tests that would catch it need a card, and the two devices in
//! CI are a software rasterizer and a virtual display controller, both of which
//! import cheerfully.

pub mod device;
pub mod kms;
pub mod output;
pub mod target;

pub use output::{CommitRequest, DmaBufPlanes, FbHandle, Mode, OutputEvent, ScanoutOutput};
pub use target::{DrmScanoutTarget, DEFAULT_RING_DEPTH};

pub use kms::KmsOutput;
