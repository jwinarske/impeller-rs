//! What fills a shape.
//!
//! A material is the paint as a backend sees it: fully resolved, in clip space,
//! and packed into the layout the shader expects. Resolving happens above,
//! because gradient geometry has to travel through the same transform the shape
//! did.

/// Floats in the packed representation.
///
/// 128 bytes, which is exactly what every device is required to offer and
/// therefore the ceiling rather than a comfortable fit. That limit is what
/// decided the stop count and the geometry budget, not the other way round: a
/// part providing only the minimum is exactly the embedded hardware this
/// renderer targets.
///
/// A material needing more than this — an image shader, with its own sampler
/// and matrix — does not belong in push constants and wants a uniform buffer.
/// Being at the limit is a signal that the next material is the one that
/// changes the mechanism.
pub const MATERIAL_FLOATS: usize = 32;

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

/// Offsets into the packed layout, matching the shader's declaration.
///
/// Public because it is a contract between the shader and every backend, not an
/// internal detail. A backend without push constants has to set each member
/// separately, and naming the offsets here keeps it from repeating the layout
/// as bare indices that quietly go stale when the layout grows.
pub mod layout {
    /// Four stop colours.
    pub const STOPS: usize = 0;
    /// Stop positions.
    pub const OFFSETS: usize = 16;
    /// Endpoints, or centre plus angles.
    pub const GEOMETRY: usize = 20;
    /// Clip-space to gradient-space matrix, in column order.
    pub const TO_LOCAL: usize = 24;
    /// Stop count and material kind.
    pub const PARAMS: usize = 28;
}

/// Kind selector shared with the shader.
pub mod kind {
    pub const SOLID: f32 = 0.0;
    pub const LINEAR: f32 = 1.0;
    pub const RADIAL: f32 = 2.0;
    pub const SWEEP: f32 = 3.0;
    pub const IMAGE: f32 = 4.0;
    pub const GLYPH: f32 = 5.0;
}

/// Tile mode selector shared with the shader.
pub mod tile {
    pub const CLAMP: f32 = 0.0;
    pub const REPEAT: f32 = 1.0;
    pub const DECAL: f32 = 2.0;
}

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

/// Maps a clip-space offset into a gradient's own space, in column order.
///
/// Clip space is anisotropic whenever the target is not square, and a transform
/// may rotate or skew as well, so a circle in user space is an ellipse there.
/// Radial and sweep gradients measure distance and angle, both of which that
/// distortion changes, so they map back before measuring. A linear gradient
/// projects onto an axis, which distortion does not affect, and so does not
/// need this.
pub type ToLocal = [f32; 4];

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
    /// A gradient outward from a centre, **in clip space**, where `to_local`
    /// carries the radius: it maps the clip-space offset so that the gradient's
    /// edge lands at unit distance.
    RadialGradient {
        center: [f32; 2],
        to_local: ToLocal,
        stops: Vec<Stop>,
    },
    /// A gradient around a centre, **in clip space**, running from `start_angle`
    /// to `end_angle` in radians.
    SweepGradient {
        center: [f32; 2],
        to_local: ToLocal,
        start_angle: f32,
        end_angle: f32,
        stops: Vec<Stop>,
    },
    /// A texture, sampled through a mapping from clip space.
    ///
    /// `origin` and `to_local` together are an affine: a clip-space position
    /// maps to texture coordinates as `to_local * (clip - origin)`, which lands
    /// the image's top-left corner at zero and its bottom-right at one. The
    /// same pair a radial gradient uses, for the same reason — clip space is
    /// anisotropic on a non-square target, so a mapping that ignored it would
    /// stretch every image by the aspect ratio.
    ///
    /// Which texture is not named here. The material is data the recorder
    /// produces without touching the device, so it carries a slot into the
    /// table supplied at submission instead of a backend handle.
    Image {
        origin: [f32; 2],
        to_local: ToLocal,
        /// Index into the texture table given at submission.
        slot: u32,
        /// Scales the sampled color, for drawing an image translucently.
        ///
        /// Applied to premultiplied color, so it scales the whole texel rather
        /// than only its alpha. A texture holds premultiplied color whether it
        /// was uploaded or rendered into, and treating a sample as straight
        /// alpha would apply the alpha twice — invisible for an opaque image,
        /// and plain the moment one translucent image is drawn into another.
        alpha: f32,
        tile: TileMode,
    },
    /// Coverage sampled from an atlas, tinting one color.
    ///
    /// Distinct from [`Self::Image`] in two ways that matter. The texture is
    /// read as *coverage* rather than as color — one channel scaling a solid,
    /// which is what antialiased text is — and the coordinates come from the
    /// vertices rather than from a mapping in the paint, so a run of glyphs
    /// reading different parts of one atlas is a single draw.
    Glyph {
        /// Linear color with straight alpha, as the text is painted.
        color: [f32; 4],
        /// Index into the texture table given at submission.
        slot: u32,
    },
}

/// What happens outside an image's own bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TileMode {
    /// Hold the edge pixel. The usual choice for drawing an image once.
    #[default]
    Clamp,
    /// Repeat the image, tiling the plane.
    Repeat,
    /// Draw nothing outside the image.
    ///
    /// Distinct from clamping in the only case that matters: a shape larger
    /// than the image it is filled with. Clamping smears the border across the
    /// remainder, which reads as a rendering fault rather than as a choice.
    Decal,
}

impl Material {
    pub fn solid(color: [f32; 4]) -> Self {
        Self::Solid(color)
    }

    /// Whether drawing with this would change anything.
    pub fn is_invisible(&self) -> bool {
        match self {
            Self::Solid(color) => color[3] <= 0.0,
            Self::LinearGradient { stops, .. }
            | Self::RadialGradient { stops, .. }
            | Self::SweepGradient { stops, .. } => {
                stops.is_empty() || stops.iter().all(|s| s.color[3] <= 0.0)
            }
            // What the texture holds is unknown here, so only a zero alpha
            // makes an image provably invisible.
            Self::Image { alpha, .. } => *alpha <= 0.0,
            Self::Glyph { color, .. } => color[3] <= 0.0,
        }
    }

    /// The texture slot this samples, for a backend building its bindings.
    pub fn texture_slot(&self) -> Option<u32> {
        match self {
            Self::Image { slot, .. } | Self::Glyph { slot, .. } => Some(*slot),
            _ => None,
        }
    }

    /// The stops, for any material that has them.
    fn stops(&self) -> &[Stop] {
        match self {
            Self::Solid(_) | Self::Image { .. } | Self::Glyph { .. } => &[],
            Self::LinearGradient { stops, .. }
            | Self::RadialGradient { stops, .. }
            | Self::SweepGradient { stops, .. } => stops,
        }
    }

    /// Pack into the layout the shader declares.
    ///
    /// Stops beyond the limit are dropped rather than resampled, and the count
    /// travels alongside so the shader ignores unused entries instead of
    /// blending toward whatever happens to be in them.
    pub fn to_push_constants(&self) -> [f32; MATERIAL_FLOATS] {
        let mut out = [0.0f32; MATERIAL_FLOATS];

        if let Self::Solid(color) = self {
            out[layout::STOPS..layout::STOPS + 4].copy_from_slice(color);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::SOLID;
            return out;
        }

        // A glyph is a solid color plus a texture read; the coordinates come
        // from the vertices, so nothing about the mapping is packed here.
        if let Self::Glyph { color, .. } = self {
            out[layout::STOPS..layout::STOPS + 4].copy_from_slice(color);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::GLYPH;
            return out;
        }

        // An image carries no stops and no count, and must be packed before
        // the gradient path below decides it has too few to interpolate.
        if let Self::Image {
            origin,
            to_local,
            alpha,
            tile,
            ..
        } = self
        {
            out[layout::GEOMETRY] = origin[0];
            out[layout::GEOMETRY + 1] = origin[1];
            out[layout::GEOMETRY + 2] = *alpha;
            out[layout::GEOMETRY + 3] = match tile {
                TileMode::Clamp => tile::CLAMP,
                TileMode::Repeat => tile::REPEAT,
                TileMode::Decal => tile::DECAL,
            };
            out[layout::TO_LOCAL..layout::TO_LOCAL + 4].copy_from_slice(to_local);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::IMAGE;
            return out;
        }

        let stops = self.stops();
        let count = stops.len().min(MAX_STOPS);
        for (i, stop) in stops.iter().take(count).enumerate() {
            out[layout::STOPS + i * 4..layout::STOPS + i * 4 + 4].copy_from_slice(&stop.color);
            out[layout::OFFSETS + i] = stop.offset;
        }
        out[layout::PARAMS] = count.max(1) as f32;

        // A gradient with fewer than two stops has nothing to interpolate
        // between, so it renders as its first colour rather than sending the
        // shader down a path that would read an entry nothing wrote.
        if count < 2 {
            out[layout::PARAMS + 1] = kind::SOLID;
            return out;
        }

        match self {
            Self::Solid(_) | Self::Image { .. } | Self::Glyph { .. } => {
                unreachable!("handled above")
            }
            Self::LinearGradient { start, end, .. } => {
                out[layout::GEOMETRY] = start[0];
                out[layout::GEOMETRY + 1] = start[1];
                out[layout::GEOMETRY + 2] = end[0];
                out[layout::GEOMETRY + 3] = end[1];
                out[layout::PARAMS + 1] = kind::LINEAR;
            }
            Self::RadialGradient {
                center, to_local, ..
            } => {
                out[layout::GEOMETRY] = center[0];
                out[layout::GEOMETRY + 1] = center[1];
                out[layout::TO_LOCAL..layout::TO_LOCAL + 4].copy_from_slice(to_local);
                out[layout::PARAMS + 1] = kind::RADIAL;
            }
            Self::SweepGradient {
                center,
                to_local,
                start_angle,
                end_angle,
                ..
            } => {
                out[layout::GEOMETRY] = center[0];
                out[layout::GEOMETRY + 1] = center[1];
                out[layout::GEOMETRY + 2] = *start_angle;
                out[layout::GEOMETRY + 3] = *end_angle;
                out[layout::TO_LOCAL..layout::TO_LOCAL + 4].copy_from_slice(to_local);
                out[layout::PARAMS + 1] = kind::SWEEP;
            }
        }
        out
    }

    /// Which shader variant this needs, for keying a pipeline.
    ///
    /// Every variant lives in one program today, selected by a uniform, so this
    /// exists for the moment a material needs its own pipeline rather than
    /// pretending that moment has arrived.
    pub fn variant(&self) -> MaterialVariant {
        match self {
            Self::Solid(_) => MaterialVariant::Solid,
            _ => MaterialVariant::Gradient,
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

    fn two_stops() -> Vec<Stop> {
        vec![
            Stop::new([1.0, 0.0, 0.0, 1.0], 0.0),
            Stop::new([0.0, 0.0, 1.0, 1.0], 1.0),
        ]
    }

    #[test]
    fn a_solid_colour_lands_in_the_first_stop_and_selects_the_solid_path() {
        let packed = Material::solid([0.25, 0.5, 0.75, 1.0]).to_push_constants();
        assert_eq!(&packed[0..4], &[0.25, 0.5, 0.75, 1.0]);
        assert_eq!(packed[layout::PARAMS], 1.0, "stop count");
        assert_eq!(packed[layout::PARAMS + 1], kind::SOLID);
    }

    #[test]
    fn a_linear_gradient_packs_its_stops_endpoints_and_count() {
        let packed = Material::LinearGradient {
            start: [-1.0, 0.0],
            end: [1.0, 0.0],
            stops: two_stops(),
        }
        .to_push_constants();

        assert_eq!(&packed[0..4], &[1.0, 0.0, 0.0, 1.0], "first stop");
        assert_eq!(&packed[4..8], &[0.0, 0.0, 1.0, 1.0], "second stop");
        assert_eq!(packed[layout::OFFSETS], 0.0);
        assert_eq!(packed[layout::OFFSETS + 1], 1.0);
        assert_eq!(
            &packed[layout::GEOMETRY..layout::GEOMETRY + 4],
            &[-1.0, 0.0, 1.0, 0.0]
        );
        assert_eq!(packed[layout::PARAMS + 1], kind::LINEAR);
    }

    #[test]
    fn a_radial_gradient_packs_its_centre_and_mapping() {
        let packed = Material::RadialGradient {
            center: [0.25, -0.5],
            to_local: [2.0, 0.0, 0.0, 4.0],
            stops: two_stops(),
        }
        .to_push_constants();

        assert_eq!(
            &packed[layout::GEOMETRY..layout::GEOMETRY + 2],
            &[0.25, -0.5]
        );
        // The mapping is what makes a circle circular on a non-square target,
        // so it has to survive packing intact.
        assert_eq!(
            &packed[layout::TO_LOCAL..layout::TO_LOCAL + 4],
            &[2.0, 0.0, 0.0, 4.0]
        );
        assert_eq!(packed[layout::PARAMS + 1], kind::RADIAL);
    }

    #[test]
    fn a_sweep_gradient_packs_its_angles_alongside_its_centre() {
        let packed = Material::SweepGradient {
            center: [0.0, 0.0],
            to_local: [1.0, 0.0, 0.0, 1.0],
            start_angle: 0.5,
            end_angle: 2.5,
            stops: two_stops(),
        }
        .to_push_constants();

        // Angles share the geometry slot with the centre, which is why a linear
        // gradient's endpoints and a sweep's angles cannot both be present.
        assert_eq!(
            &packed[layout::GEOMETRY..layout::GEOMETRY + 4],
            &[0.0, 0.0, 0.5, 2.5]
        );
        assert_eq!(packed[layout::PARAMS + 1], kind::SWEEP);
    }

    #[test]
    fn every_gradient_kind_falls_back_to_solid_with_one_stop() {
        // Interpolating needs two points. Selecting a gradient path with one
        // would have the shader read an entry nothing wrote.
        let one = vec![Stop::new([1.0, 1.0, 1.0, 1.0], 0.0)];
        let materials = [
            Material::LinearGradient {
                start: [0.0, 0.0],
                end: [1.0, 0.0],
                stops: one.clone(),
            },
            Material::RadialGradient {
                center: [0.0, 0.0],
                to_local: [1.0, 0.0, 0.0, 1.0],
                stops: one.clone(),
            },
            Material::SweepGradient {
                center: [0.0, 0.0],
                to_local: [1.0, 0.0, 0.0, 1.0],
                start_angle: 0.0,
                end_angle: 1.0,
                stops: one,
            },
        ];
        for material in materials {
            let packed = material.to_push_constants();
            assert_eq!(packed[layout::PARAMS + 1], kind::SOLID, "{material:?}");
            assert_eq!(&packed[0..4], &[1.0, 1.0, 1.0, 1.0]);
        }
    }

    #[test]
    fn stops_beyond_the_limit_are_dropped_rather_than_overflowing() {
        let stops: Vec<Stop> = (0..8)
            .map(|i| Stop::new([i as f32 / 8.0, 0.0, 0.0, 1.0], i as f32 / 7.0))
            .collect();
        let packed = Material::LinearGradient {
            start: [0.0, 0.0],
            end: [1.0, 0.0],
            stops,
        }
        .to_push_constants();
        // The count is what stops the shader reading past what was written.
        assert_eq!(packed[layout::PARAMS], MAX_STOPS as f32);
    }

    #[test]
    fn visibility_accounts_for_every_stop_of_every_kind() {
        assert!(Material::solid([1.0, 1.0, 1.0, 0.0]).is_invisible());
        assert!(!Material::solid([0.0, 0.0, 0.0, 1.0]).is_invisible());

        let clear = vec![
            Stop::new([1.0, 0.0, 0.0, 0.0], 0.0),
            Stop::new([0.0, 0.0, 1.0, 0.0], 1.0),
        ];
        assert!(Material::RadialGradient {
            center: [0.0, 0.0],
            to_local: [1.0, 0.0, 0.0, 1.0],
            stops: clear,
        }
        .is_invisible());

        let partly = vec![
            Stop::new([1.0, 0.0, 0.0, 0.0], 0.0),
            Stop::new([0.0, 0.0, 1.0, 1.0], 1.0),
        ];
        assert!(
            !Material::SweepGradient {
                center: [0.0, 0.0],
                to_local: [1.0, 0.0, 0.0, 1.0],
                start_angle: 0.0,
                end_angle: 1.0,
                stops: partly,
            }
            .is_invisible(),
            "one visible stop is enough"
        );
    }

    #[test]
    fn every_gradient_kind_reports_the_same_variant() {
        // They share one program, selected by a uniform, so keying a pipeline
        // on the kind would create three identical pipelines.
        assert_eq!(Material::solid([0.0; 4]).variant(), MaterialVariant::Solid);
        for material in [
            Material::LinearGradient {
                start: [0.0; 2],
                end: [1.0, 0.0],
                stops: two_stops(),
            },
            Material::RadialGradient {
                center: [0.0; 2],
                to_local: [1.0, 0.0, 0.0, 1.0],
                stops: two_stops(),
            },
        ] {
            assert_eq!(material.variant(), MaterialVariant::Gradient);
        }
    }
}
