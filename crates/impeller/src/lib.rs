//! impeller-rs: a tessellation-based 2D vector renderer.
//!
//! ```no_run
//! use impeller::{BackendPreference, Canvas, Color, Context, Extent2D, Paint, PixelFormat, Rect};
//!
//! let mut ctx = Context::new(BackendPreference::Auto)?;
//! let size = Extent2D::new(256, 256);
//! let mut surface = ctx.create_surface(size, PixelFormat::Rgba8Unorm)?;
//!
//! let mut canvas = Canvas::new(size);
//! canvas.clear(Color::WHITE);
//! canvas.draw_rect(Rect::new(32.0, 32.0, 224.0, 224.0), &Paint::fill(Color::rgba8(0, 120, 220, 255)))?;
//!
//! ctx.draw(&mut surface, &canvas.finish())?;
//! let pixels = ctx.read(&mut surface)?;
//! ctx.destroy_surface(surface);
//! # Ok::<(), impeller::Error>(())
//! ```
//!
//! Which backend runs is decided when the context is created, not when the
//! binary is built, so one image can boot on a board with a working Vulkan
//! driver and on one where only GLES is usable.
//!
//! Code above the backend branches on [`Context::capabilities`] rather than on
//! which backend it got. A device that cannot export a fence is the same
//! problem whichever API it speaks, and asking which backend it is gets both
//! cases wrong.

pub mod backend;
pub mod context;

pub use backend::{Backend, BackendPreference};
pub use context::{Context, Image, Surface};

// The drawing API, so an application depends on this crate alone.
pub use impeller_core::{
    Affine2, Atlas, AtlasError, AtlasRect, BlendMode, Canvas, Color, ColorFilter, ColorForm,
    Coverage, Dash, Extent2D, FillRule, Gamma, GlyphKey, GradientStop, ImageFilter, Layer, LineCap,
    LineJoin, MaskBlurStyle, Morphology, Paint, Path, PathBuilder, PixelFormat, PointMode,
    PositionedGlyph, Recording, Rect, Sampling, Shader, SourceRect, Sprite, StrokeStyle, Style,
    TileMode, Vec2, VertexMode, Vertices, MAX_STOPS,
};
pub use impeller_hal::{
    mip_levels_for, BlendFactor, Capabilities, Error, Result, RuntimeProgram, MORPHOLOGY_TAPS,
    RUNTIME_FLOATS,
};

/// Direct scanout to a display, with no compositor.
#[cfg(feature = "drm")]
pub use impeller_present_drm as drm;

/// A Vulkan WSI swapchain, for drawing into a window system's surface.
///
/// This was a feature name that enabled nothing for as long as it existed:
/// `present-wsi` was in the default set and gated no dependency, and the crate
/// implementing it was not one. So the facade's default build advertised a
/// presentation path it had no route to, and a caller wanting a window had to
/// reach past this crate to `impeller-present-vk` -- which the playground does,
/// and which was the visible symptom nobody read as one.
#[cfg(feature = "present-wsi")]
pub use impeller_present_vk as wsi;

/// An EGL window surface, which is the GLES backend's way to the same place.
#[cfg(feature = "present-egl")]
pub use impeller_present_egl as egl;

/// Presentation targets and format negotiation.
pub use impeller_present as present;
