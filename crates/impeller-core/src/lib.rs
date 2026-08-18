//! The public API: what an application draws with.
//!
//! Everything below this — tessellation, batching, backends, presentation — is
//! reachable but not usually reached for. A caller describes a frame on a
//! [`Canvas`] and hands the resulting [`Recording`] to a target.
//!
//! Recording is deliberately separate from submitting. Describing a whole frame
//! before any of it reaches the GPU is what lets draws share a single pass, and
//! it keeps the API free of the question of when work is flushed.
//!
//! Colour is linear throughout, converted at this boundary. Blending,
//! filtering, and antialiasing all average colours, and averaging sRGB-encoded
//! values is visibly wrong — see [`Color`].

pub mod canvas;
pub mod color;
pub mod execute;
pub mod paint;

pub use canvas::{Canvas, Layer, Pass, Recording, Rect, TextureSource};
pub use color::Color;
pub use execute::{execute, render_offscreen};
pub use impeller_text::{Atlas, AtlasError, AtlasRect, Coverage, GlyphKey, PositionedGlyph};
pub use paint::{GradientStop, Paint, Shader, Style};

// Geometry a caller builds paths with, re-exported so an application needs one
// dependency rather than three.
pub use impeller_geometry::stroke::{LineCap, LineJoin, StrokeStyle};
pub use impeller_geometry::{FillRule, Path, PathBuilder};
pub use impeller_hal::{BlendMode, Extent2D, PixelFormat, TileMode};

/// Two-dimensional vector and affine types, from `glam`.
pub use glam::{Affine2, Vec2};
