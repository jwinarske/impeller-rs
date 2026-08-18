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
    Affine2, Atlas, AtlasError, AtlasRect, BlendMode, Canvas, Color, Coverage, Extent2D, FillRule,
    GlyphKey, GradientStop, Layer, LineCap, LineJoin, Paint, Path, PathBuilder, PixelFormat,
    PositionedGlyph, Recording, Rect, Shader, StrokeStyle, Style, TileMode, Vec2, MAX_STOPS,
};
pub use impeller_hal::{BlendFactor, Capabilities, Error, Result};

/// Direct scanout to a display, with no compositor.
#[cfg(feature = "drm")]
pub use impeller_present_drm as drm;

/// Presentation targets and format negotiation.
pub use impeller_present as present;
