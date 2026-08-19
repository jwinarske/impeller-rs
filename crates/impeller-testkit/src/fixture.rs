//! The one texture a scene can sample.
//!
//! A scene is data that can be written down without a device, which is what
//! lets the same list serve a window, a headless comparison and a board at the
//! end of a cable. A texture handle is the opposite of that, so a scene does
//! not carry one: it says it samples an image, and the executor uploads this
//! and binds it as the only slot.
//!
//! One fixture rather than a table of them, because the alternative is a scene
//! format that has to name images and a set of images that has to be shipped
//! and kept in step. What a picture of a texture has to show is where each
//! texel landed, and one sheet chosen to make that legible does that for every
//! scene that needs it.

use impeller_hal::Extent2D;

/// The slot the executor binds the fixture to.
pub const SLOT: u32 = 0;

/// The fixture's size in texels.
pub const SIZE: Extent2D = Extent2D {
    width: 8,
    height: 8,
};

/// An eight-by-eight sheet, asymmetric in both axes and in every quadrant.
///
/// Deliberately blocky and few: what a texture scene has to show is which
/// texel a coordinate read, and a photograph would hide a transpose that this
/// makes obvious. The quadrants differ in hue and the diagonal differs from
/// its mirror, so a flip, a transpose and a rotation each produce a picture
/// that cannot be mistaken for the original.
pub fn pixels() -> Vec<u8> {
    let mut out = vec![0u8; (SIZE.area() * 4) as usize];
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            let i = ((y * SIZE.width + x) * 4) as usize;
            let left = x < SIZE.width / 2;
            let top = y < SIZE.height / 2;
            // A checker within each quadrant, so a scene that magnifies the
            // sheet shows texel boundaries and one that tiles it shows where
            // the repetitions meet.
            let dark = (x + y) % 2 == 0;
            let base: [u8; 3] = match (left, top) {
                (true, true) => [220, 40, 40],
                (false, true) => [40, 200, 90],
                (true, false) => [50, 90, 230],
                (false, false) => [230, 200, 40],
            };
            let scale = if dark { 160u32 } else { 255 };
            for c in 0..3 {
                out[i + c] = (base[c] as u32 * scale / 255) as u8;
            }
            out[i + 3] = 255;
        }
    }
    out
}
