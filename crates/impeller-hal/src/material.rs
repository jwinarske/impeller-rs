//! What fills a shape.
//!
//! A material is the paint as a backend sees it: fully resolved, in clip space,
//! and packed into the layout the shader expects. Resolving happens above,
//! because gradient endpoints have to travel through the same transform the
//! geometry did.

/// Floats in the packed representation.
///
/// 112 bytes, inside the 128 every device is required to offer. Staying within
/// the guaranteed minimum matters more than a larger stop count would: a part
/// that provides only the minimum is exactly the embedded hardware this
/// renderer targets, and a material that did not fit there would have to fall
/// back to a uniform buffer on the devices least able to afford one.
pub const MATERIAL_FLOATS: usize = 28;

/// Enforced at compile time rather than by a test, so a material that outgrew
/// the guaranteed push-constant size could not be built at all.
const _: () = assert!(
    MATERIAL_FLOATS * 4 <= 128,
    "a material must fit the 128 bytes of push constants every device guarantees"
);

/// The most stops a gradient can carry.
///
/// Four covers the overwhelming majority of real gradients. More needs the
/// stops baked into a ramp texture and sampled, which waits on the HAL growing
/// texture sampling.
pub const MAX_STOPS: usize = 4;

/// A colour stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    /// Linear colour with straight alpha.
    pub color: [f32; 4],
    /// Position along the gradient, from zero to one.
    pub offset: f32,
}

impl Stop {
    pub fn new(color: [f32; 4], offset: f32) -> Self {
        Self { color, offset }
    }
}

/// How a shape is filled.
#[derive(Debug, Clone, PartialEq)]
pub enum Material {
    Solid([f32; 4]),
    /// A gradient along the line between two points, **in clip space**.
    ///
    /// Clip space rather than user space because the fragment stage locates
    /// itself from an interpolated clip position: the alternative, the fragment
    /// coordinate builtin, has a different origin in each graphics API and
    /// would run the gradient in opposite directions on the two backends.
    LinearGradient {
        start: [f32; 2],
        end: [f32; 2],
        stops: Vec<Stop>,
    },
}

impl Material {
    pub fn solid(color: [f32; 4]) -> Self {
        Self::Solid(color)
    }

    /// The colour a fully opaque solid fill would use, for culling decisions.
    pub fn is_invisible(&self) -> bool {
        match self {
            Self::Solid(color) => color[3] <= 0.0,
            Self::LinearGradient { stops, .. } => {
                stops.is_empty() || stops.iter().all(|s| s.color[3] <= 0.0)
            }
        }
    }

    /// Pack into the layout the shader declares.
    ///
    /// Stops beyond the limit are dropped rather than resampled, and the count
    /// travels alongside so the shader ignores unused entries instead of
    /// blending toward whatever happens to be in them.
    pub fn to_push_constants(&self) -> [f32; MATERIAL_FLOATS] {
        let mut out = [0.0f32; MATERIAL_FLOATS];
        match self {
            Self::Solid(color) => {
                out[0..4].copy_from_slice(color);
                // One stop, kind zero: the shader takes the first colour and
                // never evaluates the gradient path.
                out[24] = 1.0;
                out[25] = 0.0;
            }
            Self::LinearGradient { start, end, stops } => {
                let count = stops.len().min(MAX_STOPS);
                for (i, stop) in stops.iter().take(count).enumerate() {
                    out[i * 4..i * 4 + 4].copy_from_slice(&stop.color);
                    out[16 + i] = stop.offset;
                }
                // A gradient with one stop is a solid fill, and one with none
                // would leave the shader reading uninitialized entries.
                out[20] = start[0];
                out[21] = start[1];
                out[22] = end[0];
                out[23] = end[1];
                out[24] = count.max(1) as f32;
                out[25] = if count >= 2 { 1.0 } else { 0.0 };
            }
        }
        out
    }

    /// Which shader variant this needs, for keying a pipeline.
    ///
    /// Both variants live in one program today, selected by a uniform, so this
    /// exists for the moment a material needs its own pipeline rather than
    /// pretending that moment has arrived.
    pub fn variant(&self) -> MaterialVariant {
        match self {
            Self::Solid(_) => MaterialVariant::Solid,
            Self::LinearGradient { .. } => MaterialVariant::Gradient,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaterialVariant {
    Solid,
    Gradient,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_solid_colour_lands_in_the_first_stop_and_selects_the_solid_path() {
        let packed = Material::solid([0.25, 0.5, 0.75, 1.0]).to_push_constants();
        assert_eq!(&packed[0..4], &[0.25, 0.5, 0.75, 1.0]);
        assert_eq!(packed[24], 1.0, "stop count");
        assert_eq!(packed[25], 0.0, "kind should be solid");
    }

    #[test]
    fn a_gradient_packs_its_stops_endpoints_and_count() {
        let material = Material::LinearGradient {
            start: [-1.0, 0.0],
            end: [1.0, 0.0],
            stops: vec![
                Stop::new([1.0, 0.0, 0.0, 1.0], 0.0),
                Stop::new([0.0, 0.0, 1.0, 1.0], 1.0),
            ],
        };
        let packed = material.to_push_constants();

        assert_eq!(&packed[0..4], &[1.0, 0.0, 0.0, 1.0], "first stop");
        assert_eq!(&packed[4..8], &[0.0, 0.0, 1.0, 1.0], "second stop");
        assert_eq!(packed[16], 0.0, "first offset");
        assert_eq!(packed[17], 1.0, "second offset");
        assert_eq!(&packed[20..24], &[-1.0, 0.0, 1.0, 0.0], "endpoints");
        assert_eq!(packed[24], 2.0, "stop count");
        assert_eq!(packed[25], 1.0, "kind should be gradient");
    }

    #[test]
    fn a_gradient_with_one_stop_is_treated_as_solid() {
        // Interpolating needs two points. Selecting the gradient path with one
        // would have the shader read an entry nothing wrote.
        let material = Material::LinearGradient {
            start: [0.0, 0.0],
            end: [1.0, 0.0],
            stops: vec![Stop::new([1.0, 1.0, 1.0, 1.0], 0.0)],
        };
        let packed = material.to_push_constants();
        assert_eq!(packed[25], 0.0, "kind should fall back to solid");
        assert_eq!(&packed[0..4], &[1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn stops_beyond_the_limit_are_dropped_rather_than_overflowing() {
        let stops: Vec<Stop> = (0..8)
            .map(|i| Stop::new([i as f32 / 8.0, 0.0, 0.0, 1.0], i as f32 / 7.0))
            .collect();
        let material = Material::LinearGradient {
            start: [0.0, 0.0],
            end: [1.0, 0.0],
            stops,
        };
        let packed = material.to_push_constants();
        // The count is what stops the shader reading past what was written.
        assert_eq!(packed[24], MAX_STOPS as f32);
    }

    #[test]
    fn visibility_accounts_for_every_stop() {
        assert!(Material::solid([1.0, 1.0, 1.0, 0.0]).is_invisible());
        assert!(!Material::solid([0.0, 0.0, 0.0, 1.0]).is_invisible());

        let transparent = Material::LinearGradient {
            start: [0.0, 0.0],
            end: [1.0, 0.0],
            stops: vec![
                Stop::new([1.0, 0.0, 0.0, 0.0], 0.0),
                Stop::new([0.0, 0.0, 1.0, 0.0], 1.0),
            ],
        };
        assert!(transparent.is_invisible());

        let partly = Material::LinearGradient {
            start: [0.0, 0.0],
            end: [1.0, 0.0],
            stops: vec![
                Stop::new([1.0, 0.0, 0.0, 0.0], 0.0),
                Stop::new([0.0, 0.0, 1.0, 1.0], 1.0),
            ],
        };
        assert!(!partly.is_invisible(), "one visible stop is enough");
    }
}
