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
//! # What it does on real boards
//!
//! The whole path is: render into a buffer, export it as a dma-buf, hand the fd
//! to the display controller, scan it out. Two Raspberry Pis say that works on
//! one of the three display controllers between them, and the differences are
//! worth having written down, because CI has neither the hardware nor anything
//! that would fail the way it fails.
//!
//! | board | controller | outcome |
//! |---|---|---|
//! | Pi 4 | `vc4` | the import is refused |
//! | Pi 5 | `vc4`, HDMI | it imports, the commit is refused |
//! | Pi 5 | `rp1-dsi`, DSI | **every test passes** |
//!
//! So direct scanout does work on a board, end to end, at 800x1280 over DSI on
//! a Pi 5. That is the first time anything here has been shown to reach a panel
//! rather than a virtual display controller.
//!
//! On both boards the renderer and the display are separate DRM devices --
//! `v3d` renders, something else scans out -- and `v3d` has no display role at
//! all: it carries no connectors and refuses `create_dumb_buffer` with
//! `ENOSYS`. There is no second display device to fall back to.
//!
//! ## The Pi 4: `vc4` will not take the memory
//!
//! ```text
//! driving /dev/dri/card1 at 1280x1440
//! agreed on Fourcc(875713089) with Modifier(0)
//! prime_fd_to_buffer: Invalid argument (os error 22)
//! ```
//!
//! Everything up to the handoff works, which is what makes it precise: master
//! is taken, the connector and mode are read, and negotiation agrees on `XR24`
//! with the linear modifier -- a format and layout the controller itself
//! advertised. It is the buffer that is declined, and that was shown rather
//! than inferred: a dumb buffer allocated on the same card exports and imports
//! straight back through the same call. `vc4` imports dma-bufs; it declines
//! *this* one, which leaves the memory as the only difference. A display
//! controller with no IOMMU can address only contiguous memory, and a render
//! device with an MMU has no reason to allocate that way.
//!
//! ## The Pi 5's `vc4`: it takes the memory and refuses the commit
//!
//! A different failure, and one this has not diagnosed. The import succeeds and
//! `atomic_commit` returns `EINVAL` on every frame. The mode is not obviously
//! the cause -- 1280x1440 is the first mode that connector advertises -- and no
//! more than that is known. Saying so is better than picking the likeliest of
//! several explanations and writing it down as though it had been checked.
//!
//! ## What this suggests, and what it does not
//!
//! It is not a Pi 4 limitation, and not an ARM one. Two of the three
//! controllers refuse for two different reasons and the third is fine, so the
//! shape of the problem is per-controller.
//!
//! Where the buffer comes from is the part worth reconsidering, and it is a
//! design question rather than a defect to patch. A board whose display
//! controller cannot take the renderer's memory wants the buffer allocated on
//! the *display* side and imported into the renderer, which is the opposite
//! direction to this and a different shape for [`target::DrmScanoutTarget`]
//! rather than a fix to its import call. That would cover the Pi 4. It would
//! not, on the evidence here, cover the Pi 5's HDMI.
//!
//! The tests take `IMPELLER_DRM_CARD` for this reason: a board with more than
//! one display controller otherwise gets whichever `/dev/dri` lists first,
//! which on a Pi 5 is the one that works.

pub mod device;
pub mod kms;
pub mod output;
pub mod target;

pub use output::{CommitRequest, DmaBufPlanes, FbHandle, Mode, OutputEvent, ScanoutOutput};
pub use target::{DrmScanoutTarget, DEFAULT_RING_DEPTH};

pub use kms::KmsOutput;
