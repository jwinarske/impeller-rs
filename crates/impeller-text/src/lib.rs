//! Glyph atlas and text rendering: swash rasterization into LRU atlas pages,
//! subpixel positioning, SDF above a threshold size, and COLR/CPAL color
//! glyphs as image quads.
//!
//! Text shaping and layout are out of scope -- bring cosmic-text or parley.
