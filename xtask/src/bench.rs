//! The measurement `docs/architecture.md` asks for and could not make.
//!
//! One number in that document decides a design: at 1920x1080 with a hundred
//! and sixty rounded rectangles, the analytic distance field at one sample was
//! measured at 0.26 ms against 0.64 ms for the same shapes tessellated at four
//! and 0.20 ms tessellated at one -- from which "the whole of the gain is in
//! not multisampling", and from which the field is *slower* than the triangles
//! it replaces at equal sample count. The document then says the balance is
//! hardware-dependent in the direction this project cares about, and that it
//! "should be measured on a board before the field is assumed to be the faster
//! path everywhere".
//!
//! Nothing in the tree could make that measurement. This can.
//!
//! # What it is not
//!
//! Not a regression gate. It prints numbers and returns success whatever they
//! say, because a threshold nobody has calibrated on a machine nobody has
//! characterized is a build failure waiting to happen on a busy laptop. The
//! test-lane table records regression gating as belonging to nightly runs on
//! quiet runners, and that is still where it belongs.
//!
//! Not a comparison against the numbers above either. Those were taken on a
//! desktop discrete part; a run here measures whatever this machine is, which
//! is why every result names its device.

use impeller_core::{Canvas, Color, Paint, Recording, Rect};
use impeller_hal::{Extent2D, Hal, HalContext, PixelFormat, TextureDescriptor};
use impeller_hal_gles::{DisplayTarget, GlesContext, GlesHal};
use impeller_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal};
use std::time::{Duration, Instant};

/// The frame the document's number was taken from.
const EXTENT: Extent2D = Extent2D {
    width: 1920,
    height: 1080,
};

/// A hundred and sixty rounded rectangles, as the document says.
const SHAPES: usize = 160;

/// Rendered before timing starts, so pipeline creation and the first
/// allocation of every buffer are not counted as frame cost.
const WARMUP: usize = 5;

/// Timed frames.
///
/// Two hundred rather than the thirty this began with, and the reason is the
/// percentile below. A ninety-ninth percentile of thirty samples is the
/// largest of them by another name -- `ceil(0.99 * 30)` is thirty -- so
/// reporting one would have dressed the maximum up as a distribution. Two
/// hundred puts the ninety-ninth at the third-largest, which is a tail rather
/// than an outlier, and costs about fifty milliseconds a configuration at the
/// times this actually measures.
const FRAMES: usize = 200;

/// The percentile below is only a percentile if there are samples enough for it.
///
/// At the compiler rather than in a test, because it is a statement about a
/// constant: `ceil(0.99 * n)` is `n` for every `n` under a hundred, so a p99
/// taken from fewer would be the maximum under another name and no run would
/// say so.
const _: () = assert!(FRAMES >= 100);

/// How a frame's shapes are drawn, which is the whole question.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// A rounded rectangle stated as one, which reaches the analytic distance
    /// field: coverage from an implicit function in the fragment stage, over a
    /// quad, with the pass left at one sample.
    Analytic,
    /// The same shape stated as a path, which tessellates it. Antialiased, so
    /// the pass multisamples.
    TessellatedMultisampled,
    /// The same again with antialiasing off, which leaves the pass at one
    /// sample and is the third of the document's three figures.
    TessellatedSingleSampled,
}

impl Path {
    pub fn name(self) -> &'static str {
        match self {
            Self::Analytic => "distance field, 1 sample",
            Self::TessellatedMultisampled => "tessellated, 4 samples",
            Self::TessellatedSingleSampled => "tessellated, 1 sample",
        }
    }
}

/// Lay the shapes out in a grid that fills the frame.
///
/// Placed off the whole pixel deliberately: an axis-aligned rectangle at
/// integer bounds has no edge to antialias, which is the one case where the
/// field's advantage does not exist and the comparison would flatter it.
fn shapes() -> impl Iterator<Item = Rect> {
    let columns = 16usize;
    let rows = SHAPES / columns;
    let width = EXTENT.width as f32 / columns as f32;
    let height = EXTENT.height as f32 / rows as f32;
    (0..SHAPES).map(move |i| {
        let (column, row) = (i % columns, i / columns);
        let left = column as f32 * width + 4.3;
        let top = row as f32 * height + 4.7;
        Rect::new(left, top, left + width - 8.0, top + height - 8.0)
    })
}

/// One frame's worth of drawing, recorded the way the path under test asks.
pub fn recording(path: Path) -> Recording {
    let mut canvas = Canvas::new(EXTENT);
    canvas.clear(Color::BLACK);
    let paint = Paint::fill(Color::WHITE).with_anti_alias(path != Path::TessellatedSingleSampled);
    for rect in shapes() {
        let drawn = match path {
            Path::Analytic => canvas.draw_rrect(rect, 12.0, &paint),
            // The same shape and the same paint, stated as a path so that the
            // tessellator sees it rather than the fragment stage. Comparing
            // the two forms of one shape is what makes this a measurement of
            // the path rather than of the content.
            _ => canvas.draw_path(&rect.to_rounded_path(12.0), &paint),
        };
        drawn.expect("a rounded rectangle");
    }
    canvas.finish()
}

/// What a run of one configuration on one device came to.
pub struct Timing {
    pub device: String,
    pub path: Path,
    /// How many draws the frame came to.
    ///
    /// Reported because it is not the same between the two paths and the
    /// difference is not incidental. Tessellated shapes all carry one solid
    /// material, so the batch merges them into a single draw; an analytic one
    /// carries its own geometry in its material and cannot merge with its
    /// neighbor. The same hundred and sixty shapes are one draw down one path
    /// and a hundred and sixty down the other, and a timing that did not say so
    /// would look like a comparison of fragment work.
    pub draws: usize,
    /// The middle frame, which is what to read: a mean folds in whatever else
    /// the machine was doing during the slowest one.
    pub median: Duration,
    /// The frame ninety-nine hundredths of them came in under.
    ///
    /// Reported beside the median because that pair is what a frame budget is
    /// written against -- `plan.md`'s desktop target is a rate *and* a p99
    /// within twice the median, and a renderer that hits an average while
    /// missing one frame in fifty is not the same thing as one that does not.
    /// The slowest frame is a different statement: one interruption owns it,
    /// and on a machine doing anything else there is always one.
    pub p99: Duration,
    pub fastest: Duration,
    pub slowest: Duration,
}

impl Timing {
    /// Whether the spread is wide enough that the median should not be trusted
    /// on its own.
    ///
    /// Reported rather than acted on. A shared machine gives noisy answers and
    /// the honest response is to say so next to the number, not to retry until
    /// it looks quiet.
    pub fn noisy(&self) -> bool {
        self.slowest > self.fastest * 2
    }

    /// Frames a second, if every frame took the median.
    ///
    /// A rate is what the targets are written in and a duration is what was
    /// measured, so the conversion belongs here rather than in a reader's head.
    pub fn rate(&self) -> f64 {
        let seconds = self.median.as_secs_f64();
        if seconds > 0.0 {
            1.0 / seconds
        } else {
            f64::INFINITY
        }
    }
}

/// The sample `fraction` of the way through a sorted run, by nearest rank.
///
/// Nearest rank rather than an interpolation: every value here is a frame that
/// happened, and a p99 that lands between two of them is a number no frame
/// took. Panics on an empty slice, which the caller cannot produce -- the run
/// that fills it returns `None` before this is reached.
fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Time one configuration, forcing the device to finish each frame.
///
/// `finish` is what makes the number a frame rather than a submission. The
/// Vulkan path already waits on a fence inside the submit, so its barrier is
/// nothing; the GLES path returns as soon as the commands are queued, and
/// timing that measured the driver's willingness to accept work -- a hundred
/// and sixty rounded rectangles at this size came to nineteen microseconds,
/// with an occasional frame a hundred times that when the queue backed up.
fn time_frames<H: Hal>(
    ctx: &mut H::Context,
    device: String,
    path: Path,
    recording: &Recording,
    finish: fn(&mut H::Context),
) -> Option<Timing>
where
    H::Context: HalContext<Hal = H>,
{
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(
            EXTENT,
            PixelFormat::Rgba8Unorm,
        ))
        .ok()?;

    let mut run = |count: usize| -> Option<Vec<Duration>> {
        let mut samples = Vec::with_capacity(count);
        for _ in 0..count {
            let started = Instant::now();
            let outcome = impeller_core::execute::<H>(ctx, &mut target, recording, &[]);
            finish(ctx);
            samples.push(started.elapsed());
            outcome.ok()?;
        }
        Some(samples)
    };

    let outcome = run(WARMUP).and_then(|_| run(FRAMES));
    ctx.destroy_texture(target);

    let mut samples = outcome?;
    samples.sort();
    Some(Timing {
        device,
        path,
        draws: recording.draw_count(),
        median: percentile(&samples, 0.5),
        p99: percentile(&samples, 0.99),
        fastest: samples[0],
        slowest: samples[samples.len() - 1],
    })
}

/// Run every configuration on every device this machine offers.
pub fn gather() -> Vec<Timing> {
    let mut timings = Vec::new();
    let paths = [
        Path::Analytic,
        Path::TessellatedMultisampled,
        Path::TessellatedSingleSampled,
    ];

    for index in 0.. {
        let Ok(mut ctx) = VulkanContext::new(DevicePreference::Index(index)) else {
            break;
        };
        let device = format!("vulkan:{index} {}", ctx.capabilities().device_name);
        for path in paths {
            let recording = recording(path);
            // Nothing: the Vulkan submit waits on its own fence before it
            // returns, so the frame is already over when the clock stops.
            if let Some(timing) =
                time_frames::<VulkanHal>(&mut ctx, device.clone(), path, &recording, |_| {})
            {
                timings.push(timing);
            }
        }
    }

    if let Ok(mut ctx) = GlesContext::new(DisplayTarget::Surfaceless) {
        let device = format!("gles {}", ctx.capabilities().device_name);
        for path in paths {
            let recording = recording(path);
            if let Some(timing) =
                time_frames::<GlesHal>(&mut ctx, device.clone(), path, &recording, |ctx| {
                    // SAFETY: a context is current on this thread.
                    unsafe { glow::HasContext::finish(ctx.raw_gl()) }
                })
            {
                timings.push(timing);
            }
        }
    }
    timings
}

fn millis(duration: Duration) -> String {
    format!("{:.3} ms", duration.as_secs_f64() * 1000.0)
}

pub fn text(timings: &[Timing]) -> String {
    if timings.is_empty() {
        return "no device on this machine could render the frame\n".to_string();
    }
    let mut out = format!(
        "{SHAPES} rounded rectangles at {}x{}, {FRAMES} frames after {WARMUP} warm-up\n",
        EXTENT.width, EXTENT.height
    );
    let mut current = String::new();
    for timing in timings {
        if timing.device != current {
            current.clone_from(&timing.device);
            out.push_str(&format!("\n{current}\n"));
        }
        out.push_str(&format!(
            "  {:<24} {:>10} ({:>6.0} fps)   p99 {:>10}   fastest {:>10}   \
             slowest {:>10}   {:>4} draws{}\n",
            timing.path.name(),
            millis(timing.median),
            timing.rate(),
            millis(timing.p99),
            millis(timing.fastest),
            millis(timing.slowest),
            timing.draws,
            if timing.noisy() { "   (noisy)" } else { "" }
        ));
    }
    out.push_str(
        "\nMedians, with the frame ninety-nine hundredths came in under beside \n\
         them. Nothing here passes or fails: these are what this machine did, \n\
         and the balance between the two paths is hardware-dependent by design \n\
         -- see the distance-field section of docs/architecture.md.\n\
         \n\
         The rate is what a median frame would sustain with nothing else in it: \n\
         no present, no vertical blank, and a scene that is a hundred and sixty \n\
         rectangles rather than an interface. Read it against the other paths \n\
         here rather than against a target in `plan.md`, which names a different \n\
         scene and counts a whole frame.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two paths do not cost the same number of draws, and the difference
    /// is the first thing to know about the comparison rather than a detail.
    ///
    /// This was written expecting them to match, on the reasoning that the same
    /// shapes drawn two ways should record the same work. They do not. Every
    /// tessellated shape carries the same solid material, so the batch merges
    /// all hundred and sixty into one draw; an analytic shape carries its own
    /// geometry inside its material and can merge with nothing. So the timing
    /// below is not fragment work against fragment work -- it is one draw of
    /// many triangles against a hundred and sixty draws of two, and reading it
    /// as anything else would be reading it wrong.
    #[test]
    fn the_two_paths_differ_in_draw_count_and_the_output_says_so() {
        let draws = |path| recording(path).draw_count();
        assert_eq!(
            draws(Path::Analytic),
            SHAPES,
            "an analytic shape carries its own geometry, so none of them merge"
        );
        assert_eq!(
            draws(Path::TessellatedMultisampled),
            1,
            "tessellated shapes share one solid material and merge into one draw"
        );
        assert_eq!(draws(Path::TessellatedSingleSampled), 1);

        // And the number reaches the reader, because a comparison that hid it
        // would look like a measurement of shading alone.
        let rendered = text(&[Timing {
            device: "test".into(),
            path: Path::Analytic,
            draws: SHAPES,
            median: Duration::from_micros(100),
            p99: Duration::from_micros(140),
            fastest: Duration::from_micros(90),
            slowest: Duration::from_micros(110),
        }]);
        assert!(rendered.contains("160 draws"), "{rendered}");
    }

    #[test]
    fn every_configuration_draws_the_same_shapes() {
        // What does hold: the geometry is the same in all three, which is what
        // makes the comparison about the path rather than about the content.
        let first: Vec<_> = shapes().collect();
        assert_eq!(first.len(), SHAPES);
        assert!(first
            .iter()
            .all(|r| r.width() > 0.0 && r.height() > 0.0 && r.right <= EXTENT.width as f32));
    }

    #[test]
    fn a_percentile_is_a_frame_that_happened() {
        // Nearest rank, so every answer is a sample rather than a point
        // between two of them: these are frames that were rendered, and a
        // duration nothing took is not a frame time.
        let run: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        assert_eq!(percentile(&run, 0.5), Duration::from_millis(50));
        assert_eq!(percentile(&run, 0.99), Duration::from_millis(99));
        assert_eq!(percentile(&run, 1.0), Duration::from_millis(100));
        // A length the fractions do not divide, which is what tells nearest
        // rank from the alternatives. At a hundred samples `0.5 * 100` is a
        // whole number and every rounding agrees; at seven it is three and a
        // half, and rounding it down picks the third of seven where the rank is
        // the fourth. The first version of this test used only round hundreds
        // and passed with the rounding reversed.
        let seven: Vec<Duration> = (1..=7).map(Duration::from_millis).collect();
        assert_eq!(percentile(&seven, 0.5), Duration::from_millis(4));
        assert_eq!(percentile(&seven, 0.99), Duration::from_millis(7));
        assert_eq!(percentile(&seven, 0.25), Duration::from_millis(2));

        // And the ends hold: nothing indexes past either edge.
        assert_eq!(percentile(&run, 0.0), Duration::from_millis(1));
        assert_eq!(
            percentile(&[Duration::from_millis(7)], 0.99),
            Duration::from_millis(7)
        );
    }

    #[test]
    fn a_ninety_ninth_percentile_is_not_the_slowest_frame() {
        // The reason `FRAMES` is what it is. At thirty samples `ceil(0.99 * 30)`
        // is thirty, so a p99 would be the maximum wearing another name -- one
        // interruption, reported as a distribution. At two hundred it is the
        // third from the end, which a single stall cannot reach.
        let mut run: Vec<Duration> = vec![Duration::from_millis(1); FRAMES];
        let last = run.len() - 1;
        run[last] = Duration::from_millis(500);
        assert_eq!(
            percentile(&run, 0.99),
            Duration::from_millis(1),
            "one stalled frame in {FRAMES} reached the ninety-ninth percentile"
        );
    }

    #[test]
    fn a_shape_sits_off_the_pixel_grid() {
        // An axis-aligned rectangle at integer bounds has no edge to
        // antialias, which is the one case where the field has no advantage to
        // measure. Placing the grid on whole pixels would quietly flatter it.
        let first = shapes().next().expect("a shape");
        assert!(first.left.fract() != 0.0 && first.top.fract() != 0.0);
    }

    #[test]
    fn an_empty_run_says_so_rather_than_printing_a_bare_header() {
        assert!(text(&[]).contains("no device"));
    }

    #[test]
    fn a_wide_spread_is_reported_rather_than_hidden() {
        let timing = |fastest: u64, slowest: u64| Timing {
            device: "test".into(),
            path: Path::Analytic,
            draws: SHAPES,
            median: Duration::from_micros((fastest + slowest) / 2),
            p99: Duration::from_micros(slowest),
            fastest: Duration::from_micros(fastest),
            slowest: Duration::from_micros(slowest),
        };
        assert!(timing(100, 500).noisy());
        assert!(!timing(100, 150).noisy());
        assert!(text(&[timing(100, 500)]).contains("noisy"));
    }
}
