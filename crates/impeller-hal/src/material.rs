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
    /// Four stop colors.
    pub const STOPS: usize = 0;
    /// Stop positions.
    pub const OFFSETS: usize = 16;
    /// Endpoints, or center plus angles.
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
    pub const BLUR: f32 = 6.0;
    pub const ROUNDED_RECT: f32 = 7.0;
    pub const ELLIPSE: f32 = 8.0;
}

/// Tile mode selector shared with the shader.
pub mod tile {
    pub const CLAMP: f32 = 0.0;
    pub const REPEAT: f32 = 1.0;
    pub const DECAL: f32 = 2.0;
}

/// A color stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    /// Linear color with straight alpha.
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
    /// A gradient along an axis, starting at a point **in clip space**.
    ///
    /// Clip space for the start, because the fragment stage locates itself from
    /// an interpolated clip position: the alternative, the fragment coordinate
    /// builtin, has a different origin in each graphics API and would run the
    /// gradient in opposite directions on the two backends.
    ///
    /// The axis is in the gradient's own space, and `to_local` maps a clip-space
    /// offset into it — the same pairing radial and sweep use, and for the same
    /// reason. Clip space is anisotropic whenever the target is not square, so
    /// projecting onto an axis *there* weights the two axes by the target's
    /// shape: on a target twice as wide as it is tall, a diagonal gradient runs
    /// in the wrong direction.
    LinearGradient {
        start: [f32; 2],
        /// End minus start, in the gradient's own space.
        axis: [f32; 2],
        to_local: ToLocal,
        stops: Vec<Stop>,
    },
    /// A gradient outward from a center, **in clip space**, where `to_local`
    /// carries the radius: it maps the clip-space offset so that the gradient's
    /// edge lands at unit distance.
    RadialGradient {
        center: [f32; 2],
        to_local: ToLocal,
        stops: Vec<Stop>,
    },
    /// A gradient around a center, **in clip space**, running from `start_angle`
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
    /// A rounded rectangle evaluated per fragment rather than tessellated.
    ///
    /// The shape an interface is mostly made of, and the one where computing
    /// coverage beats building triangles for it. A tessellated rounded
    /// rectangle costs vertices proportional to how round it is and has hard
    /// edges unless the whole pass is multisampled; this is two triangles
    /// whatever the radius, and antialiases itself from the distance field it
    /// already computes.
    ///
    /// The geometry is in the shape's own space, with `to_local` mapping a
    /// clip-space position into it -- the same pairing the gradients use, and
    /// for the same reason: clip space is anisotropic on a target that is not
    /// square, and a distance measured there would round the corners by
    /// different amounts on each axis.
    RoundedRect {
        color: [f32; 4],
        /// Where the fragment stage locates the shape, in clip space.
        center: [f32; 2],
        /// Half the width and height, in the shape's own space.
        half_size: [f32; 2],
        to_local: ToLocal,
        /// Corner radius, in the shape's own space.
        radius: f32,
        /// Trace the outline at this width rather than filling, in the shape's
        /// own space. Zero fills.
        ///
        /// Costs a distance field nothing: the field already says how far every
        /// fragment is from the edge, so an outline is the band where that is
        /// small. Tessellating one instead means building a second shape --
        /// offset inward and outward, with the corners resolved -- which is
        /// where a stroked outline gets its vertex count and its joins.
        stroke: f32,
    },
    /// An ellipse evaluated per fragment.
    ///
    /// Its own variant rather than a rounded rectangle with a large radius,
    /// which gives a stadium: past half the shorter side a rounded rectangle
    /// stops changing, where an ellipse keeps curving along both axes.
    ///
    /// The same geometry a rounded rectangle carries, less the radius, which
    /// the two axes already state.
    Ellipse {
        color: [f32; 4],
        /// Where the fragment stage locates the shape, in clip space.
        center: [f32; 2],
        /// The two semi-axes, in the shape's own space.
        half_size: [f32; 2],
        to_local: ToLocal,
        /// Trace the outline at this width rather than filling. Zero fills.
        stroke: f32,
    },
    /// One axis of a separable Gaussian blur of a sampled texture.
    ///
    /// Separable because a two-dimensional Gaussian is the product of two
    /// one-dimensional ones, so blurring along each axis in turn gives the
    /// same result as a square of taps at a fraction of the cost: at a radius
    /// of sixteen that is thirty-three taps against a thousand and eighty-nine.
    /// Two passes are the price, which is why this names an axis rather than
    /// describing the whole blur.
    Blur {
        origin: [f32; 2],
        to_local: ToLocal,
        slot: u32,
        /// One tap's step, in the sampled texture's own coordinates.
        ///
        /// The axis and the texel size together: a horizontal pass over a
        /// target `w` wide steps `(1/w, 0)`. Stated here rather than derived in
        /// the shader because the shader does not know the size of what it is
        /// sampling.
        step: [f32; 2],
        /// Standard deviation, in taps.
        ///
        /// The tap count follows from it -- three deviations each way covers
        /// better than four nines of the curve -- so a caller sets how soft the
        /// result is and nothing else.
        sigma: f32,
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
            // A blur of nothing is nothing, but the pass still has to run: what
            // it samples is not knowable from here.
            Self::Blur { .. } => false,
            Self::RoundedRect {
                color, half_size, ..
            }
            | Self::Ellipse {
                color, half_size, ..
            } => color[3] <= 0.0 || half_size[0] <= 0.0 || half_size[1] <= 0.0,
            Self::Glyph { color, .. } => color[3] <= 0.0,
        }
    }

    /// The texture slot this samples, for a backend building its bindings.
    ///
    /// Matched exhaustively rather than with a catch-all. A variant that
    /// samples something and is not listed here reports no slot, so the backend
    /// binds its placeholder and the draw comes out flat white -- a plausible
    /// picture rather than an error, and one nothing else would explain.
    pub fn texture_slot(&self) -> Option<u32> {
        match self {
            Self::Image { slot, .. } | Self::Glyph { slot, .. } | Self::Blur { slot, .. } => {
                Some(*slot)
            }
            Self::Solid(_)
            | Self::LinearGradient { .. }
            | Self::RadialGradient { .. }
            | Self::SweepGradient { .. }
            | Self::RoundedRect { .. }
            | Self::Ellipse { .. } => None,
        }
    }

    /// The stops, for any material that has them.
    fn stops(&self) -> &[Stop] {
        match self {
            Self::Solid(_)
            | Self::Image { .. }
            | Self::Glyph { .. }
            | Self::Blur { .. }
            | Self::RoundedRect { .. }
            | Self::Ellipse { .. } => &[],
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
        if let Self::Ellipse {
            color,
            center,
            half_size,
            to_local,
            stroke,
        } = self
        {
            out[layout::STOPS..layout::STOPS + 4].copy_from_slice(color);
            out[layout::GEOMETRY] = center[0];
            out[layout::GEOMETRY + 1] = center[1];
            out[layout::GEOMETRY + 2] = half_size[0];
            out[layout::GEOMETRY + 3] = half_size[1];
            out[layout::TO_LOCAL..layout::TO_LOCAL + 4].copy_from_slice(to_local);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::ELLIPSE;
            out[layout::PARAMS + 3] = *stroke;
            return out;
        }

        if let Self::RoundedRect {
            color,
            center,
            half_size,
            to_local,
            radius,
            stroke,
        } = self
        {
            out[layout::STOPS..layout::STOPS + 4].copy_from_slice(color);
            out[layout::GEOMETRY] = center[0];
            out[layout::GEOMETRY + 1] = center[1];
            out[layout::GEOMETRY + 2] = half_size[0];
            out[layout::GEOMETRY + 3] = half_size[1];
            out[layout::TO_LOCAL..layout::TO_LOCAL + 4].copy_from_slice(to_local);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::ROUNDED_RECT;
            out[layout::PARAMS + 2] = *radius;
            out[layout::PARAMS + 3] = *stroke;
            return out;
        }

        if let Self::Blur {
            origin,
            to_local,
            step,
            sigma,
            ..
        } = self
        {
            out[layout::GEOMETRY] = origin[0];
            out[layout::GEOMETRY + 1] = origin[1];
            out[layout::GEOMETRY + 2] = step[0];
            out[layout::GEOMETRY + 3] = step[1];
            out[layout::TO_LOCAL..layout::TO_LOCAL + 4].copy_from_slice(to_local);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::BLUR;
            out[layout::PARAMS + 2] = *sigma;
            return out;
        }

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
        // between, so it renders as its first color rather than sending the
        // shader down a path that would read an entry nothing wrote.
        if count < 2 {
            out[layout::PARAMS + 1] = kind::SOLID;
            return out;
        }

        match self {
            Self::Solid(_)
            | Self::Image { .. }
            | Self::Glyph { .. }
            | Self::Blur { .. }
            | Self::RoundedRect { .. }
            | Self::Ellipse { .. } => {
                unreachable!("handled above")
            }
            Self::LinearGradient {
                start,
                axis,
                to_local,
                ..
            } => {
                out[layout::GEOMETRY] = start[0];
                out[layout::GEOMETRY + 1] = start[1];
                out[layout::GEOMETRY + 2] = axis[0];
                out[layout::GEOMETRY + 3] = axis[1];
                out[layout::TO_LOCAL..layout::TO_LOCAL + 4].copy_from_slice(to_local);
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
    fn a_linear_gradient_packs_its_stops_start_axis_and_count() {
        let packed = Material::LinearGradient {
            start: [-1.0, 0.0],
            axis: [2.0, 0.0],
            to_local: [1.0, 0.0, 0.0, 1.0],
            stops: two_stops(),
        }
        .to_push_constants();

        assert_eq!(&packed[0..4], &[1.0, 0.0, 0.0, 1.0], "first stop");
        assert_eq!(&packed[4..8], &[0.0, 0.0, 1.0, 1.0], "second stop");
        assert_eq!(packed[layout::OFFSETS], 0.0);
        assert_eq!(packed[layout::OFFSETS + 1], 1.0);
        assert_eq!(
            &packed[layout::GEOMETRY..layout::GEOMETRY + 4],
            &[-1.0, 0.0, 2.0, 0.0],
            "the start, then the axis rather than the end point"
        );
        assert_eq!(
            &packed[layout::TO_LOCAL..layout::TO_LOCAL + 4],
            &[1.0, 0.0, 0.0, 1.0]
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

        // Angles share the geometry slot with the center, which is why a linear
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
                axis: [1.0 - 0.0, 0.0 - 0.0],
                to_local: [1.0, 0.0, 0.0, 1.0],
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
            axis: [1.0 - 0.0, 0.0 - 0.0],
            to_local: [1.0, 0.0, 0.0, 1.0],
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
                axis: [1.0, 0.0],
                to_local: [1.0, 0.0, 0.0, 1.0],
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
