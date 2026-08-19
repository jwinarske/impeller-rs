//! Baking a gradient's stops into a texture the shader can sample.
//!
//! # Why a texture at all
//!
//! The material carries a fixed, small number of stops in push constants, which
//! covers the overwhelming majority of real gradients and nothing beyond them.
//! A gradient with more used to be truncated, silently; then it was refused,
//! honestly. This is the third answer: evaluate the ramp once on the way in and
//! hand the shader an image of it, so the count stops mattering.
//!
//! # Why sRGB, and why straight alpha
//!
//! The ramp is stored encoded and sampled through an sRGB format, so the device
//! decodes it back to linear on the way out. Eight bits of *linear* color band
//! visibly in the darks — the eye's resolution is not uniform across the range,
//! which is the entire reason the transfer function exists — while eight bits
//! spaced by that function is what every image file in the world uses and is
//! enough for a color ramp.
//!
//! The color is stored straight rather than premultiplied. A transfer function
//! is nonlinear, so encoding a premultiplied value and decoding it does not
//! give back the premultiplied value; the multiplication has to happen after
//! the decode. That matches what the shader already does with stops read from
//! push constants, which is to premultiply at the very end, so the ramp path
//! and the four-stop path converge before anything acts on the color.
//!
//! Alpha is not subject to the transfer function in an sRGB format — it stays
//! linear — which is what makes storing it beside encoded color correct rather
//! than merely convenient.

use crate::paint::GradientStop;

/// How many texels a baked ramp gets.
///
/// A gradient is a one-dimensional function sampled with a linear filter, so
/// the question is how finely it has to be tabulated before interpolation
/// between neighbors is indistinguishable from evaluating it. At this width a
/// texel spans well under one percent of the ramp, which is finer than eight
/// bits can express a difference across, so the sampling is not the limit.
pub const RAMP_WIDTH: usize = 256;

/// A gradient's colors, tabulated and encoded for upload.
///
/// Owned by the recording rather than by the caller, because the recorder is
/// what knows the stops and nothing outside it should have to bake them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ramp {
    /// `RAMP_WIDTH` texels, RGBA, color encoded and alpha linear.
    pub texels: Vec<u8>,
}

impl Ramp {
    /// Tabulate `stops` across the width.
    ///
    /// The stops are taken as given: in order, and covering whatever part of
    /// the range they cover. Before the first and after the last the ramp holds
    /// the end colors, which is not a tile mode — tiling happens to the
    /// parameter before it reaches the ramp, so by here the question is only
    /// what a parameter inside the table means.
    pub fn bake(stops: &[GradientStop]) -> Self {
        let mut texels = vec![0u8; RAMP_WIDTH * 4];
        for (i, texel) in texels.chunks_exact_mut(4).enumerate() {
            // Sampled at texel centers, because that is where a linear filter
            // reads them: treating the first texel as t=0 would shift the whole
            // ramp by half a texel against the four-stop path, which is exactly
            // the kind of difference that shows up as a seam between a gradient
            // and one drawn beside it with fewer stops.
            let t = (i as f32 + 0.5) / RAMP_WIDTH as f32;
            let color = sample_at(stops, t);
            // Color through the transfer function, alpha not: an sRGB format
            // encodes three channels and leaves the fourth alone, so writing
            // alpha encoded here would be decoded as though it never had been.
            let encoded = color.to_srgb();
            texel[0] = quantize(encoded[0]);
            texel[1] = quantize(encoded[1]);
            texel[2] = quantize(encoded[2]);
            texel[3] = quantize(color.a);
        }
        Self { texels }
    }
}

fn quantize(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The color a gradient shows at `t`, from stops in order.
///
/// Deliberately the same walk the shader performs over its four stops, so the
/// two paths agree where they overlap. A gradient of four stops drawn through
/// push constants and the same one drawn through a ramp must not differ; that
/// they use the same rule is what makes it true rather than approximately true.
fn sample_at(stops: &[GradientStop], t: f32) -> crate::Color {
    let Some(first) = stops.first() else {
        return crate::Color::linear(0.0, 0.0, 0.0, 0.0);
    };
    let mut result = first.color;
    for pair in stops.windows(2) {
        let (lower, upper) = (&pair[0], &pair[1]);
        let span = (upper.offset - lower.offset).max(1e-6);
        let local = ((t - lower.offset) / span).clamp(0.0, 1.0);
        if t >= lower.offset {
            result = mix(lower.color, upper.color, local);
        }
    }
    result
}

fn mix(a: crate::Color, b: crate::Color, t: f32) -> crate::Color {
    crate::Color::linear(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a + (b.a - a.a) * t,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    fn stops(n: usize) -> Vec<GradientStop> {
        (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                GradientStop::new(Color::linear(t, 0.0, 1.0 - t, 1.0), t)
            })
            .collect()
    }

    fn texel(ramp: &Ramp, i: usize) -> [u8; 4] {
        let at = i * 4;
        [
            ramp.texels[at],
            ramp.texels[at + 1],
            ramp.texels[at + 2],
            ramp.texels[at + 3],
        ]
    }

    #[test]
    fn a_ramp_is_the_full_width_and_fully_written() {
        let ramp = Ramp::bake(&stops(6));
        assert_eq!(ramp.texels.len(), RAMP_WIDTH * 4);
        // Every alpha is opaque here, so a texel nothing wrote would show as a
        // hole rather than blend in with its neighbors.
        assert!(
            ramp.texels.chunks_exact(4).all(|t| t[3] == 255),
            "a texel was left unwritten"
        );
    }

    #[test]
    fn the_ends_hold_the_first_and_last_stop() {
        let ramp = Ramp::bake(&stops(6));
        let first = texel(&ramp, 0);
        let last = texel(&ramp, RAMP_WIDTH - 1);
        // Not exactly the stop colors: the first texel's center is half a texel
        // in, so it has already traveled that far along the ramp. Close, and
        // unmistakably at the right end of it.
        assert!(first[2] > 240 && first[0] < 16, "first texel {first:?}");
        assert!(last[0] > 240 && last[2] < 16, "last texel {last:?}");
    }

    #[test]
    fn every_stop_appears_where_it_was_placed() {
        // Three stops at a quarter, half and three quarters, each a channel of
        // its own, so a ramp that placed them evenly instead of where they were
        // asked for would put the wrong color at each probe.
        let stops = vec![
            GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.25),
            GradientStop::new(Color::linear(0.0, 1.0, 0.0, 1.0), 0.5),
            GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 0.75),
        ];
        let ramp = Ramp::bake(&stops);
        let at = |t: f32| {
            texel(
                &ramp,
                ((t * RAMP_WIDTH as f32) as usize).min(RAMP_WIDTH - 1),
            )
        };
        assert!(at(0.25)[0] > 200, "the red stop is not at a quarter");
        assert!(at(0.5)[1] > 200, "the green stop is not at the half");
        assert!(at(0.75)[2] > 200, "the blue stop is not at three quarters");
        // And before the first stop it holds that stop rather than fading in
        // from nothing, which is what clamping means inside the table.
        assert!(at(0.05)[0] > 200, "the ramp faded in before its first stop");
    }

    #[test]
    fn the_ramp_is_monotonic_where_the_stops_are() {
        // Two stops from black to white. Every texel must be at least as bright
        // as the one before it: a ramp that wrapped, reversed, or sampled out
        // of order would break this without necessarily looking wrong at any
        // single probe.
        let ramp = Ramp::bake(&[
            GradientStop::new(Color::linear(0.0, 0.0, 0.0, 1.0), 0.0),
            GradientStop::new(Color::linear(1.0, 1.0, 1.0, 1.0), 1.0),
        ]);
        let mut previous = 0u8;
        for i in 0..RAMP_WIDTH {
            let value = texel(&ramp, i)[0];
            assert!(
                value >= previous,
                "texel {i} is {value} after {previous}, so the ramp goes backwards"
            );
            previous = value;
        }
        assert!(previous > 250, "the ramp never reached white");
    }

    #[test]
    fn color_is_encoded_and_alpha_is_not() {
        // Half linear intensity encodes to about 188, which is the whole reason
        // for storing the ramp this way; half alpha stays at about 128, because
        // an sRGB format leaves the fourth channel alone and encoding it here
        // would be decoded as though it never had been.
        let ramp = Ramp::bake(&[
            GradientStop::new(Color::linear(0.5, 0.5, 0.5, 0.5), 0.0),
            GradientStop::new(Color::linear(0.5, 0.5, 0.5, 0.5), 1.0),
        ]);
        let t = texel(&ramp, RAMP_WIDTH / 2);
        assert!(
            t[0].abs_diff(188) <= 2,
            "color should be encoded, got {}",
            t[0]
        );
        assert!(
            t[3].abs_diff(128) <= 2,
            "alpha should stay linear, got {}",
            t[3]
        );
    }

    #[test]
    fn no_stops_bakes_to_nothing_rather_than_to_a_panic() {
        // Reachable only through a paint that carries no stops, which does not
        // draw -- but a bake that indexed into an empty slice would take the
        // process down on the way to finding that out.
        let ramp = Ramp::bake(&[]);
        assert_eq!(ramp.texels.len(), RAMP_WIDTH * 4);
        assert!(ramp.texels.iter().all(|b| *b == 0));
    }
}
