//! Glyph atlas and text rendering: swash rasterization into LRU atlas pages,
//! subpixel positioning, SDF above a threshold size, and COLR/CPAL color
//! glyphs as image quads.
//!
//! Text shaping and layout are out of scope -- bring cosmic-text or parley.

pub mod atlas;

pub use atlas::{Atlas, AtlasError, AtlasRect, Coverage, GlyphKey};

/// One glyph, placed.
///
/// The position is the glyph's top-left corner in user space, and the size is
/// what it covers there. Both come from the caller: shaping decides where
/// glyphs go and this does not shape. Keeping the destination separate from the
/// atlas rectangle is what lets a glyph be drawn at a size other than the one
/// it was rasterized at, which a caller doing so knowingly may want.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub key: GlyphKey,
    pub position: [f32; 2],
    pub size: [f32; 2],
}

impl PositionedGlyph {
    /// A glyph drawn at the size its coverage was rasterized at.
    pub fn new(key: GlyphKey, position: [f32; 2], rect: AtlasRect) -> Self {
        Self {
            key,
            position,
            size: [rect.width as f32, rect.height as f32],
        }
    }
}
