//! Captured images and how they are compared.
//!
//! Comparison is tolerance-based rather than exact because the specification
//! permits latitude in places: converting a blended result to normalized
//! fixed-point may take either of the two nearest representable values, so two
//! conformant drivers can differ by one unit. A tolerance is a statement about
//! where the specification allows a difference, not a way to make a failing
//! comparison pass.

/// An RGBA8 image read back from a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, premultiplied.
    pub pixels: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            pixels,
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    pub fn pixel_count(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }
}

/// How much difference is acceptable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tolerance {
    /// Largest acceptable difference in any one channel.
    pub per_channel: u8,
    /// Fraction of pixels allowed to exceed `per_channel`.
    ///
    /// Zero for most comparisons. A budget above zero belongs to scene classes
    /// where a driver's rasterization or filtering is permitted to differ at
    /// edges, and raising one should be as deliberate as changing a golden.
    pub outlier_fraction: f32,
}

impl Tolerance {
    /// Bit-exact. Correct for clears, coverage, and anything else the
    /// specification pins down.
    pub const EXACT: Self = Self {
        per_channel: 0,
        outlier_fraction: 0.0,
    };

    /// One unit per channel, no outliers. Correct where a blended result is
    /// converted to fixed-point.
    pub const ROUNDING: Self = Self {
        per_channel: 1,
        outlier_fraction: 0.0,
    };

    pub const fn new(per_channel: u8, outlier_fraction: f32) -> Self {
        Self {
            per_channel,
            outlier_fraction,
        }
    }
}

/// What a comparison found.
#[derive(Debug, Clone, PartialEq)]
pub struct Difference {
    pub max_delta: u8,
    /// Pixels exceeding the per-channel tolerance.
    pub outliers: usize,
    pub total: usize,
    /// Where the largest difference was, for pointing at the failure.
    pub worst_at: Option<(u32, u32)>,
}

impl Difference {
    pub fn outlier_fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.outliers as f32 / self.total as f32
        }
    }

    pub fn is_identical(&self) -> bool {
        self.max_delta == 0
    }
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // "differing" rather than "outside tolerance": this counts every pixel
        // that is not identical, and whether that is acceptable is a separate
        // question the caller answers with a tolerance. Saying otherwise made
        // passing comparisons read like failures.
        write!(
            f,
            "max delta {}, {} of {} pixels differing ({:.4}%)",
            self.max_delta,
            self.outliers,
            self.total,
            self.outlier_fraction() * 100.0
        )?;
        if let Some((x, y)) = self.worst_at {
            write!(f, ", worst at ({x}, {y})")?;
        }
        Ok(())
    }
}

/// Compare two images.
///
/// Returns the difference either way; the caller decides whether it passes via
/// [`accepts`], so a passing comparison can still report how close it came.
pub fn compare(a: &Image, b: &Image) -> Result<Difference, String> {
    if a.width != b.width || a.height != b.height {
        return Err(format!(
            "size mismatch: {}x{} against {}x{}",
            a.width, a.height, b.width, b.height
        ));
    }
    if a.pixels.len() != b.pixels.len() {
        return Err("pixel buffers differ in length".into());
    }

    let mut max_delta = 0u8;
    let mut worst_at = None;
    let mut outliers = 0usize;
    // Recorded per comparison rather than passed in, so the same difference can
    // be judged against different tolerances without re-scanning.
    let mut per_pixel_max = vec![0u8; a.pixel_count()];

    for (i, (pa, pb)) in a
        .pixels
        .chunks_exact(4)
        .zip(b.pixels.chunks_exact(4))
        .enumerate()
    {
        let delta = pa
            .iter()
            .zip(pb)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        per_pixel_max[i] = delta;
        if delta > max_delta {
            max_delta = delta;
            worst_at = Some((i as u32 % a.width, i as u32 / a.width));
        }
    }
    for delta in &per_pixel_max {
        if *delta > 0 {
            outliers += 1;
        }
    }

    Ok(Difference {
        max_delta,
        outliers,
        total: a.pixel_count(),
        worst_at,
    })
}

/// Whether a difference is within tolerance.
pub fn accepts(difference: &Difference, tolerance: Tolerance) -> bool {
    if difference.max_delta <= tolerance.per_channel {
        return true;
    }
    difference.outlier_fraction() <= tolerance.outlier_fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, color: [u8; 4]) -> Image {
        Image::new(
            width,
            height,
            color
                .iter()
                .copied()
                .cycle()
                .take((width * height * 4) as usize)
                .collect(),
        )
    }

    #[test]
    fn identical_images_differ_by_nothing() {
        let a = solid(4, 4, [10, 20, 30, 255]);
        let d = compare(&a, &a).unwrap();
        assert!(d.is_identical());
        assert_eq!(d.outliers, 0);
        assert!(accepts(&d, Tolerance::EXACT));
    }

    #[test]
    fn a_one_unit_difference_fails_exact_and_passes_rounding() {
        let a = solid(4, 4, [10, 20, 30, 255]);
        let b = solid(4, 4, [10, 21, 30, 255]);
        let d = compare(&a, &b).unwrap();
        assert_eq!(d.max_delta, 1);
        // The distinction the tolerance profiles exist to make.
        assert!(!accepts(&d, Tolerance::EXACT));
        assert!(accepts(&d, Tolerance::ROUNDING));
    }

    #[test]
    fn an_outlier_budget_admits_a_few_bad_pixels_but_not_many() {
        let a = solid(10, 10, [0, 0, 0, 255]);
        let mut b = a.clone();
        // Two pixels of a hundred, well outside per-channel tolerance.
        for i in 0..2 {
            b.pixels[i * 4] = 200;
        }
        let d = compare(&a, &b).unwrap();
        assert_eq!(d.max_delta, 200);
        assert_eq!(d.outliers, 2);

        assert!(accepts(&d, Tolerance::new(1, 0.05)));
        assert!(!accepts(&d, Tolerance::new(1, 0.01)));
    }

    #[test]
    fn the_worst_pixel_is_located() {
        let a = solid(4, 4, [0, 0, 0, 255]);
        let mut b = a.clone();
        // Row 2, column 1.
        b.pixels[(2 * 4 + 1) * 4] = 99;
        let d = compare(&a, &b).unwrap();
        assert_eq!(d.worst_at, Some((1, 2)));
    }

    #[test]
    fn mismatched_sizes_are_an_error_rather_than_a_difference() {
        // Comparing images of different sizes means the harness is confused
        // about what it rendered, which is not a tolerance question.
        let a = solid(4, 4, [0; 4]);
        let b = solid(8, 4, [0; 4]);
        assert!(compare(&a, &b).is_err());
    }
}
