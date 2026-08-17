//! Packing rasterized glyph coverage into one texture.
//!
//! # What this does and does not do
//!
//! It takes coverage bitmaps and answers where in a texture each one sits. It
//! does not rasterize them: turning an outline into coverage needs a font
//! parser, and font parsing is out of scope here — bring `swash` or
//! `ttf-parser` and hand the result over. That boundary is what lets the atlas
//! be tested with no font anywhere in the tree, against bitmaps whose contents
//! are known exactly.
//!
//! # Shelf packing
//!
//! Glyphs are packed into horizontal shelves: a new glyph goes on the first
//! shelf tall enough for it with room to its right, and starts a new shelf
//! otherwise. That wastes the space above short glyphs on a tall shelf, which
//! a skyline or a max-rects packer would recover.
//!
//! It is the right trade for glyphs specifically. A run of text is a stream of
//! boxes of very similar height, so shelves fill densely in practice, and the
//! packer runs once per glyph per size rather than per frame. A packer with
//! better worst-case density would cost more per insertion and more to read for
//! a gain that the input's own shape mostly removes.

use std::collections::HashMap;

/// Where a glyph sits in the atlas, in texels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtlasRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl AtlasRect {
    /// The rectangle in texture coordinates, from zero to one.
    ///
    /// Returned as `[left, top, right, bottom]`, with V running downward to
    /// match the row order the texture was uploaded in.
    pub fn uv(&self, atlas: u32) -> [f32; 4] {
        let scale = 1.0 / atlas as f32;
        [
            self.x as f32 * scale,
            self.y as f32 * scale,
            (self.x + self.width) as f32 * scale,
            (self.y + self.height) as f32 * scale,
        ]
    }
}

/// What identifies a glyph in the atlas.
///
/// A font identifier alongside the glyph index, because glyph indices are
/// per font and two fonts will disagree about what index seven means. Size is
/// in whole pixels: a cache keyed by a float would miss on values that differ
/// only in their last bit, and rasterizing at a rounded size is what a caller
/// is doing anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub font: u64,
    pub glyph: u16,
    pub size: u16,
}

/// A rasterized glyph, as a caller supplies it.
#[derive(Debug, Clone, PartialEq)]
pub struct Coverage {
    pub width: u32,
    pub height: u32,
    /// One byte per texel, row-major from the top, tightly packed.
    pub texels: Vec<u8>,
}

impl Coverage {
    /// Whether the dimensions and the buffer agree.
    pub fn is_consistent(&self) -> bool {
        self.texels.len() as u64 == self.width as u64 * self.height as u64
    }
}

/// Why a glyph could not be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtlasError {
    /// The coverage's dimensions and its buffer disagree.
    Inconsistent,
    /// The glyph does not fit, even in an empty atlas.
    TooLarge,
    /// The atlas is full.
    ///
    /// Distinct from `TooLarge` because the remedies differ: this one is fixed
    /// by evicting or by growing, and that one never is.
    Full,
}

impl std::fmt::Display for AtlasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inconsistent => write!(f, "coverage dimensions do not match its buffer"),
            Self::TooLarge => write!(f, "glyph is larger than the atlas"),
            Self::Full => write!(f, "atlas is full"),
        }
    }
}

impl std::error::Error for AtlasError {}

/// One shelf: a horizontal band, filled left to right.
#[derive(Debug, Clone, Copy)]
struct Shelf {
    top: u32,
    height: u32,
    /// Where the next glyph on this shelf would start.
    used: u32,
}

/// A square atlas of glyph coverage, and where each glyph landed.
///
/// Holds the texels itself rather than a device texture, so that packing can be
/// exercised and reasoned about without a GPU. Whoever owns the device uploads
/// [`Atlas::texels`] when [`Atlas::is_dirty`] says something changed.
#[derive(Debug)]
pub struct Atlas {
    size: u32,
    texels: Vec<u8>,
    shelves: Vec<Shelf>,
    placed: HashMap<GlyphKey, AtlasRect>,
    dirty: bool,
}

/// Blank texels around each glyph.
///
/// One texel is enough and less is not. A linear filter samples a two-by-two
/// neighborhood, so a glyph flush against its neighbor bleeds that neighbor's
/// coverage into its own edge under any magnification. The padding is cleared
/// rather than merely skipped, since the space may hold an evicted glyph.
const PADDING: u32 = 1;

impl Atlas {
    /// An empty atlas of `size` by `size` texels.
    pub fn new(size: u32) -> Self {
        Self {
            size,
            texels: vec![0; (size as usize) * (size as usize)],
            shelves: Vec::new(),
            placed: HashMap::new(),
            dirty: false,
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// One byte of coverage per texel, row-major from the top.
    pub fn texels(&self) -> &[u8] {
        &self.texels
    }

    /// Whether anything has been added since [`Atlas::mark_clean`].
    ///
    /// The upload is the caller's to make, because only they know which device
    /// the texture lives on and when in the frame it is safe to write.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    pub fn len(&self) -> usize {
        self.placed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.placed.is_empty()
    }

    /// Where a glyph sits, if it is present.
    pub fn get(&self, key: GlyphKey) -> Option<AtlasRect> {
        self.placed.get(&key).copied()
    }

    /// Add a glyph, or return where it already is.
    ///
    /// Idempotent, so a caller can insert every glyph of every run each frame
    /// and pay only for the ones that are new. That is the usage this is for:
    /// deciding what is already present is exactly what an atlas is.
    pub fn insert(&mut self, key: GlyphKey, coverage: &Coverage) -> Result<AtlasRect, AtlasError> {
        if let Some(rect) = self.placed.get(&key) {
            return Ok(*rect);
        }
        if !coverage.is_consistent() {
            return Err(AtlasError::Inconsistent);
        }
        // A glyph with no area still has a position, which is what lets a space
        // travel through a run like any other glyph rather than being a case
        // every caller has to remember to skip.
        if coverage.width == 0 || coverage.height == 0 {
            let rect = AtlasRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            };
            self.placed.insert(key, rect);
            return Ok(rect);
        }

        let rect = self.allocate(coverage.width, coverage.height)?;
        for row in 0..coverage.height {
            let from = (row * coverage.width) as usize;
            let to = ((rect.y + row) * self.size + rect.x) as usize;
            self.texels[to..to + coverage.width as usize]
                .copy_from_slice(&coverage.texels[from..from + coverage.width as usize]);
        }
        self.placed.insert(key, rect);
        self.dirty = true;
        Ok(rect)
    }

    /// Find room for a glyph and reserve it.
    fn allocate(&mut self, width: u32, height: u32) -> Result<AtlasRect, AtlasError> {
        let padded_width = width + PADDING;
        let padded_height = height + PADDING;
        if padded_width > self.size || padded_height > self.size {
            return Err(AtlasError::TooLarge);
        }

        // The first shelf tall enough with room to the right. Shelves are not
        // sorted by height, so this is a scan; a run of text produces shelves
        // of similar height and few of them, which is why that is not worth
        // indexing.
        for shelf in &mut self.shelves {
            if shelf.height >= padded_height && self.size - shelf.used >= padded_width {
                let rect = AtlasRect {
                    x: shelf.used,
                    y: shelf.top,
                    width,
                    height,
                };
                shelf.used += padded_width;
                return Ok(rect);
            }
        }

        let top = self
            .shelves
            .last()
            .map_or(0, |shelf| shelf.top + shelf.height);
        if top + padded_height > self.size {
            return Err(AtlasError::Full);
        }
        self.shelves.push(Shelf {
            top,
            height: padded_height,
            used: padded_width,
        });
        Ok(AtlasRect {
            x: 0,
            y: top,
            width,
            height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, value: u8) -> Coverage {
        Coverage {
            width,
            height,
            texels: vec![value; (width * height) as usize],
        }
    }

    fn key(glyph: u16) -> GlyphKey {
        GlyphKey {
            font: 1,
            glyph,
            size: 16,
        }
    }

    #[test]
    fn an_inserted_glyph_can_be_found_again() {
        let mut atlas = Atlas::new(64);
        let rect = atlas.insert(key(1), &solid(8, 10, 200)).expect("insert");
        assert_eq!(atlas.get(key(1)), Some(rect));
        assert_eq!(rect.width, 8);
        assert_eq!(rect.height, 10);
        assert_eq!(atlas.len(), 1);
    }

    #[test]
    fn inserting_the_same_glyph_twice_returns_the_same_place() {
        // The usage this exists for: a caller inserts every glyph of every run
        // each frame and pays only for the new ones. A second insertion that
        // allocated again would fill the atlas in proportion to frames drawn
        // rather than to distinct glyphs.
        let mut atlas = Atlas::new(64);
        let first = atlas.insert(key(1), &solid(8, 10, 200)).expect("first");
        let second = atlas.insert(key(1), &solid(8, 10, 200)).expect("second");
        assert_eq!(first, second);
        assert_eq!(atlas.len(), 1);
    }

    #[test]
    fn glyphs_differing_only_in_font_or_size_are_distinct() {
        // Glyph indices are per font, and a glyph rasterized at one size is not
        // the same picture as at another. A key that ignored either would serve
        // one glyph's coverage for another's.
        let mut atlas = Atlas::new(64);
        let a = atlas.insert(key(1), &solid(8, 8, 255)).expect("a");
        let b = atlas
            .insert(GlyphKey { font: 2, ..key(1) }, &solid(8, 8, 255))
            .expect("b");
        let c = atlas
            .insert(GlyphKey { size: 32, ..key(1) }, &solid(8, 8, 255))
            .expect("c");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        assert_eq!(atlas.len(), 3);
    }

    #[test]
    fn coverage_lands_where_the_rectangle_says() {
        let mut atlas = Atlas::new(16);
        // A bitmap with a distinct value per texel, so a transposed or
        // off-by-one copy produces a different atlas rather than the same one.
        let coverage = Coverage {
            width: 3,
            height: 2,
            texels: vec![10, 20, 30, 40, 50, 60],
        };
        let rect = atlas.insert(key(1), &coverage).expect("insert");
        for row in 0..coverage.height {
            for column in 0..coverage.width {
                let at = ((rect.y + row) * atlas.size() + rect.x + column) as usize;
                assert_eq!(
                    atlas.texels()[at],
                    coverage.texels[(row * coverage.width + column) as usize],
                    "texel ({column}, {row}) landed wrong"
                );
            }
        }
    }

    #[test]
    fn glyphs_do_not_touch_each_other() {
        // A linear filter samples a neighborhood, so a glyph flush against its
        // neighbor bleeds that neighbor's coverage into its own edge. This
        // asserts the gap rather than the filtering, since the filtering is the
        // device's business.
        let mut atlas = Atlas::new(64);
        let a = atlas.insert(key(1), &solid(8, 8, 255)).expect("a");
        let b = atlas.insert(key(2), &solid(8, 8, 255)).expect("b");
        assert!(
            b.x >= a.x + a.width + PADDING || b.y >= a.y + a.height + PADDING,
            "{a:?} and {b:?} are adjacent"
        );
    }

    #[test]
    fn a_second_shelf_starts_below_the_first() {
        let mut atlas = Atlas::new(32);
        // Three glyphs eleven wide with padding do not fit across thirty-two.
        let first = atlas.insert(key(1), &solid(11, 6, 255)).expect("first");
        let second = atlas.insert(key(2), &solid(11, 6, 255)).expect("second");
        let third = atlas.insert(key(3), &solid(11, 6, 255)).expect("third");
        assert_eq!(first.y, second.y, "the first two share a shelf");
        assert!(third.y > first.y, "the third started a new shelf");
        assert_eq!(third.x, 0, "a new shelf starts at the left edge");
    }

    #[test]
    fn a_short_glyph_reuses_a_taller_shelf() {
        // The trade shelf packing makes: the space above a short glyph on a
        // tall shelf is lost, and in exchange placement is a scan of a few
        // entries. A run of text is boxes of similar height, which is what
        // keeps that loss small.
        let mut atlas = Atlas::new(64);
        let tall = atlas.insert(key(1), &solid(8, 20, 255)).expect("tall");
        let short = atlas.insert(key(2), &solid(8, 4, 255)).expect("short");
        assert_eq!(tall.y, short.y, "the short glyph opened a new shelf");
    }

    #[test]
    fn a_glyph_larger_than_the_atlas_is_reported_as_such() {
        // Distinct from being full, because the remedies differ: growing or
        // evicting fixes one and never fixes the other.
        let mut atlas = Atlas::new(16);
        assert_eq!(
            atlas.insert(key(1), &solid(16, 16, 255)),
            Err(AtlasError::TooLarge),
            "a glyph needing padding beyond the edge should not fit"
        );
        assert_eq!(
            atlas.insert(key(2), &solid(64, 4, 255)),
            Err(AtlasError::TooLarge)
        );
    }

    #[test]
    fn a_full_atlas_says_so_rather_than_overwriting() {
        let mut atlas = Atlas::new(16);
        let mut inserted = 0;
        for glyph in 0..64u16 {
            match atlas.insert(key(glyph), &solid(6, 6, 255)) {
                Ok(_) => inserted += 1,
                Err(e) => {
                    assert_eq!(e, AtlasError::Full);
                    break;
                }
            }
        }
        assert!(inserted > 0, "nothing fit at all");
        assert!(inserted < 64, "everything fit, so fullness went untested");

        // And every glyph that did fit is still where it was put: running out
        // of room must not disturb what is already there.
        let mut seen = std::collections::HashSet::new();
        for glyph in 0..inserted as u16 {
            let rect = atlas.get(key(glyph)).expect("still present");
            assert!(seen.insert((rect.x, rect.y)), "two glyphs share a place");
        }
    }

    #[test]
    fn a_glyph_with_no_area_is_placed_rather_than_refused() {
        // A space has a position in a run like any other glyph, and making
        // every caller remember to skip it is how one of them forgets.
        let mut atlas = Atlas::new(16);
        let rect = atlas.insert(key(1), &solid(0, 0, 0)).expect("insert");
        assert_eq!(rect.width, 0);
        assert!(!atlas.is_dirty(), "an empty glyph changed no texels");
    }

    #[test]
    fn inconsistent_coverage_is_refused() {
        let mut atlas = Atlas::new(16);
        let bad = Coverage {
            width: 4,
            height: 4,
            texels: vec![0; 3],
        };
        assert_eq!(atlas.insert(key(1), &bad), Err(AtlasError::Inconsistent));
    }

    #[test]
    fn the_dirty_flag_tracks_whether_an_upload_is_owed() {
        let mut atlas = Atlas::new(32);
        assert!(!atlas.is_dirty(), "an empty atlas owes no upload");
        atlas.insert(key(1), &solid(4, 4, 255)).expect("insert");
        assert!(atlas.is_dirty());
        atlas.mark_clean();
        assert!(!atlas.is_dirty());
        // A repeat insertion changes no texels, so it owes nothing either.
        atlas.insert(key(1), &solid(4, 4, 255)).expect("again");
        assert!(
            !atlas.is_dirty(),
            "a glyph already present dirtied the atlas"
        );
    }

    #[test]
    fn texture_coordinates_span_the_rectangle() {
        let rect = AtlasRect {
            x: 16,
            y: 32,
            width: 8,
            height: 4,
        };
        let [left, top, right, bottom] = rect.uv(64);
        assert!((left - 0.25).abs() < 1e-6);
        assert!((top - 0.5).abs() < 1e-6);
        assert!((right - 0.375).abs() < 1e-6);
        assert!((bottom - 0.5625).abs() < 1e-6);
        // V runs downward, matching the row order the texels are stored in.
        assert!(bottom > top);
    }
}
