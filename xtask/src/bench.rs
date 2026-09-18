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
//! Not a regression gate *by default*, and the distinction is the whole of the
//! design. A plain run prints numbers and returns success whatever they say,
//! because a threshold nobody has calibrated on a machine nobody has
//! characterized is a build failure waiting to happen on a busy laptop.
//!
//! `--record <path>` writes what a run measured, keyed by device and
//! configuration; `--check <path>` measures again and compares, reporting every
//! row and exiting non-zero if any is slower than `--tolerance` (one percent by
//! default) or if either side has a row the other lacks. Both are opt-in, so
//! the objection above still holds for every run that did not ask for one.
//!
//! The default tolerance is measured rather than chosen, and so is the reason
//! it is not one number for every row. On a Raspberry Pi 5's V3D, seven of the
//! eight rows repeat to within three tenths of a percent across thirty-one
//! runs. The eighth -- the Vulkan distance-field figure -- lands in one of two
//! states three percent apart from one process to the next, because recording
//! a command costs differently in each; `docs/on-a-board.md` has the numbers.
//!
//! A single tolerance has to be as loose as the worst row it covers, so that
//! one row set the bar for all eight and a three percent regression on any of
//! the other seven passed unremarked. A baseline row may therefore carry its
//! own tolerance as an optional fourth field, which puts the slack on the row
//! that needs it, next to the measurement that justifies it, and lets the
//! default be tight enough to mean something.
//!
//! This does not make a busy machine a quiet one. Checking an unchanged build
//! against its own baseline on the workstation this was written on reported
//! three of eight rows regressed, one of them by twelve percent. The test-lane
//! table still records gating as belonging to nightly runs on quiet runners;
//! what has changed is that the machinery now exists for one.
//!
//! Not a comparison against the numbers above either. Those were taken on a
//! desktop discrete part; a run here measures whatever this machine is, which
//! is why every result names its device.
//!
//! And not a whole frame's work: every recording here is built once, before the
//! clock starts, and only `execute` is timed. An application records a frame
//! per frame, so whatever it costs to *build* a recording is invisible to every
//! number this prints. Worth knowing the size of that blind spot rather than
//! only that it exists -- on the machine this was written on, building the
//! full frame takes 0.007 ms and building the hundred and sixty tessellated
//! shapes 0.077 ms, against frame times of one to thirty. So it is under a
//! percent here and would be a few on a slower processor, which is small
//! enough to leave outside and too large to forget.

use impeller_core::{
    Canvas, Color, GradientStop, Layer, Paint, Recording, Rect, Shader, TileMode, Vec2,
};
use impeller_hal::{Extent2D, Hal, HalContext, PixelFormat, TextureDescriptor};
use impeller_hal_gles::{DisplayTarget, GlesContext, GlesHal};
use impeller_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal};
use std::io::{self, Write};
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
    /// The same shape *stroked*, through the analytic field: a distance to the
    /// outline rather than to the interior, still one sample.
    ///
    /// Strokes were timed by nothing at all until this, which is how a shader
    /// change that cost two and a half per cent reached a release. They are
    /// their own pair of routes and deserve their own comparison: what an
    /// outline costs is not what a fill costs, and the tessellated one has to
    /// build two contours where a fill builds one.
    StrokedAnalytic,
    /// The stroked shape as a path, which sends it through the stroker.
    /// Antialiasing off, so this and the field above are one sample each and
    /// the difference between them is the route.
    StrokedTessellated,
}

impl Path {
    pub fn name(self) -> &'static str {
        match self {
            Self::Analytic => "distance field, 1 sample",
            Self::TessellatedMultisampled => "tessellated, 4 samples",
            Self::TessellatedSingleSampled => "tessellated, 1 sample",
            Self::StrokedAnalytic => "stroked field, 1 sample",
            Self::StrokedTessellated => "stroked path, 1 sample",
        }
    }

    /// Whether this path strokes rather than fills.
    fn strokes(self) -> bool {
        matches!(self, Self::StrokedAnalytic | Self::StrokedTessellated)
    }

    /// Whether the shape goes to the tessellator rather than the field.
    fn tessellates(self) -> bool {
        matches!(
            self,
            Self::TessellatedMultisampled
                | Self::TessellatedSingleSampled
                | Self::StrokedTessellated
        )
    }
}

/// Wide enough that the stroke is geometry rather than the thin-stroke rule.
///
/// A width under a device pixel is widened to one and dimmed to pay for it,
/// which is a different measurement and a much smaller one -- what these two
/// rows are for is what an outline costs when there is an outline. Four device
/// pixels at this frame size, on shapes a hundred and twenty across.
const STROKE_WIDTH: f32 = 4.0;

/// Under a device pixel the width is widened to one and dimmed to pay for it,
/// which is a different measurement and a much smaller one. Held here so that
/// lowering the constant fails the build rather than quietly changing what the
/// two rows mean.
const _: () = assert!(STROKE_WIDTH >= 1.0);

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
    // The field route needs antialiasing -- its coverage *is* the distance, and
    // the analytic path declines a paint that asked for none -- while the
    // tessellated rows turn it off to stay at one sample. So this is which
    // route the path names rather than a per-path flag.
    let anti_alias = !matches!(
        path,
        Path::TessellatedSingleSampled | Path::StrokedTessellated
    );
    let paint = match path.strokes() {
        true => Paint::stroke(Color::WHITE, STROKE_WIDTH),
        false => Paint::fill(Color::WHITE),
    }
    .with_anti_alias(anti_alias);
    for rect in shapes() {
        let drawn = match path.tessellates() {
            // The same shape and the same paint, stated as a path so that the
            // tessellator sees it rather than the fragment stage. Comparing
            // the two forms of one shape is what makes this a measurement of
            // the path rather than of the content.
            true => canvas.draw_path(&rect.to_rounded_path(12.0), &paint),
            false => canvas.draw_rrect(rect, 12.0, &paint),
        };
        drawn.expect("a rounded rectangle");
    }
    canvas.finish()
}

/// One configuration's result, as the event that reports it either way.
fn outcome(what: &'static str, timed: Result<Timing, String>) -> Event {
    match timed {
        Ok(timing) => Event::Measured(timing),
        Err(why) => Event::Failed { what, why },
    }
}

/// How the full-frame row names itself.
const FRAME: &str = "full frame, mixed content";

/// What a whole frame of mixed content costs, which is a budget rather than a
/// comparison.
///
/// The three routes above answer one narrow question and answer it well: the
/// same shapes, twice, so the difference is the route. That is not a frame. It
/// has one material, no layer, no blur and no gradient, so it says nothing
/// about what an interface costs — and a renderer can be quick at a hundred
/// and sixty identical rectangles and slow at everything a real frame is made
/// of.
///
/// So this is the other kind: a ground that is a gradient, cards that carry
/// shadows, and a blurred layer over the top, at the size a display actually
/// is. Every one of those reaches machinery the comparison never touches — the
/// ramp, the blur's passes, the layer's own target and its composite back.
///
/// Deliberately *not* the panel example's frame, which it otherwise resembles.
/// That one turns antialiasing off on its ground because `execute_deferred`
/// cannot submit a multisampled pass, which is a constraint of presenting to a
/// display and not of drawing. A frame written to be timed should look like a
/// frame, so this leaves it on.
///
/// Static, at one instant of that scene rather than a moving one: a benchmark
/// that changed its own content between runs would report the content.
fn full_frame() -> Recording {
    let (w, h) = (EXTENT.width as f32, EXTENT.height as f32);
    let mut canvas = Canvas::new(EXTENT);
    canvas.clear(Color::srgb(0.05, 0.06, 0.09, 1.0));

    // A wash behind everything. A gradient rather than a flat fill because it
    // is the one thing here that tabulates a ramp.
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, w, h),
            &Paint::default().with_shader(Shader::LinearGradient {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(w, h),
                // Five, and the count is the point rather than the picture:
                // at or below `MAX_STOPS` a gradient travels inside the
                // material and tabulates nothing. Past it the recorder bakes a
                // ramp and the shader samples it, which is the path a frame
                // should be timed on and the one two stops would miss.
                stops: (0..5)
                    .map(|i| {
                        let t = i as f32 / 4.0;
                        GradientStop {
                            offset: t,
                            color: Color::srgb(0.08 + 0.14 * t, 0.10, 0.18 + 0.02 * t, 1.0),
                        }
                    })
                    .collect(),
                tile: TileMode::Clamp,
            }),
        )
        .expect("the ground");

    let center = Vec2::new(w * 0.5, h * 0.5);
    let orbit = w.min(h) * 0.26;
    let side = w.min(h) * 0.20;
    let hues = [
        Color::srgb(0.98, 0.42, 0.28, 1.0),
        Color::srgb(0.36, 0.82, 0.62, 1.0),
        Color::srgb(0.42, 0.58, 0.98, 1.0),
    ];
    for (i, hue) in hues.into_iter().enumerate() {
        let phase = i as f32 * std::f32::consts::TAU / 3.0;
        let at = Vec2::new(
            center.x + orbit * phase.cos(),
            center.y + orbit * phase.sin() * 0.55,
        );
        let card = Rect::new(
            at.x - side * 0.5,
            at.y - side * 0.5,
            at.x + side * 0.5,
            at.y + side * 0.5,
        );
        // Shadow first and card over it, which is the order every real caller
        // uses and the one the occluder flag describes.
        canvas
            .draw_shadow(&card.to_rounded_path(side * 0.18), Color::BLACK, 8.0, false)
            .expect("a shadow");
        canvas
            .draw_rrect(card, side * 0.18, &Paint::fill(hue))
            .expect("a card");
    }

    // A blurred highlight, so the layer's own target and its composite back are
    // in the number too.
    canvas.save_layer(Layer::opacity(0.5).with_blur(24.0));
    canvas
        .draw_circle(
            center,
            w.min(h) * 0.10,
            &Paint::fill(Color::srgb(1.0, 0.95, 0.80, 1.0)),
        )
        .expect("a highlight");
    canvas.restore();

    canvas.finish()
}

/// What a run of one configuration on one device came to.
pub struct Timing {
    /// What was timed, already spelled for the reader.
    ///
    /// A label rather than a [`Path`] for the reason the device is not here
    /// either: not everything this times is one of the two routes. A whole
    /// frame of mixed content is a budget rather than a comparison, and a
    /// field that could only say which route it was would have to lie about
    /// it.
    pub what: &'static str,
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
    what: &'static str,
    recording: &Recording,
    finish: fn(&mut H::Context),
) -> Result<Timing, String>
where
    H::Context: HalContext<Hal = H>,
{
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(
            EXTENT,
            PixelFormat::Rgba8Unorm,
        ))
        .map_err(|e| format!("no target: {e}"))?;

    let mut run = |count: usize| -> Result<Vec<Duration>, String> {
        let mut samples = Vec::with_capacity(count);
        for _ in 0..count {
            let started = Instant::now();
            let outcome = impeller_core::execute::<H>(ctx, &mut target, recording, &[]);
            finish(ctx);
            samples.push(started.elapsed());
            outcome.map_err(|e| e.to_string())?;
        }
        Ok(samples)
    };

    let outcome = run(WARMUP).and_then(|_| run(FRAMES));
    ctx.destroy_texture(target);

    let mut samples = outcome?;
    samples.sort();
    Ok(Timing {
        what,
        draws: recording.draw_count(),
        median: percentile(&samples, 0.5),
        p99: percentile(&samples, 0.99),
        fastest: samples[0],
        slowest: samples[samples.len() - 1],
    })
}

/// Run every configuration on every device this machine offers, handing each
/// result to `report` as soon as it is measured.
///
/// The observer rather than a returned vector because a full run takes minutes
/// on a slow device, and one that is interrupted partway should be a partial
/// result rather than no result at all. Collecting everything and formatting at
/// the end lost three runs on a board that left the network mid-run, each of
/// which had measured several configurations and printed none of them.
/// Whether `device` is one the caller asked to leave alone.
///
/// A substring rather than an exact name because a device names itself at
/// length and differently on every driver -- `vulkan:1 llvmpipe (LLVM 22.1.8,
/// 256 bits)` here and a different parenthesis on the next machine -- so a
/// caller who has to reproduce one exactly will get it wrong and be told
/// nothing. Case-insensitive for the same reason.
fn skipped(device: &str, skip: &[String]) -> bool {
    let device = device.to_lowercase();
    skip.iter().any(|s| device.contains(&s.to_lowercase()))
}

/// `skip` names devices to open and then leave alone, matched as
/// case-insensitive substrings of the device name. It exists because a device
/// can be measurable and still not safe to measure: a software rasterizer at
/// this size holds every core of a small board flat out for minutes, and a
/// Raspberry Pi 5 locked up at that stage of the run four times in a row,
/// having come through the hardware stages before it each time. Skipping it
/// leaves the board's own GPU measurable, which is the number worth having.
pub fn gather(skip: &[String], report: &mut dyn FnMut(Event)) {
    let paths = [
        Path::Analytic,
        Path::TessellatedMultisampled,
        Path::TessellatedSingleSampled,
        Path::StrokedAnalytic,
        Path::StrokedTessellated,
    ];

    for index in 0.. {
        let Ok(mut ctx) = VulkanContext::new(DevicePreference::Index(index)) else {
            break;
        };
        let device = format!("vulkan:{index} {}", ctx.capabilities().device_name);
        if skipped(&device, skip) {
            report(Event::Skipped(device));
            continue;
        }
        report(Event::Device(device));
        for path in paths {
            let recording = recording(path);
            // Nothing: the Vulkan submit waits on its own fence before it
            // returns, so the frame is already over when the clock stops.
            report(outcome(
                path.name(),
                time_frames::<VulkanHal>(&mut ctx, path.name(), &recording, |_| {}),
            ));
        }
        let frame = full_frame();
        report(outcome(
            FRAME,
            time_frames::<VulkanHal>(&mut ctx, FRAME, &frame, |_| {}),
        ));
    }

    if let Ok(mut ctx) = GlesContext::new(DisplayTarget::Surfaceless) {
        let device = format!("gles {}", ctx.capabilities().device_name);
        if skipped(&device, skip) {
            report(Event::Skipped(device));
            return;
        }
        report(Event::Device(device));
        for path in paths {
            let recording = recording(path);
            let timed = time_frames::<GlesHal>(&mut ctx, path.name(), &recording, |ctx| {
                // SAFETY: a context is current on this thread.
                unsafe { glow::HasContext::finish(ctx.raw_gl()) }
            });
            report(outcome(path.name(), timed));
        }
        let frame = full_frame();
        let timed = time_frames::<GlesHal>(&mut ctx, FRAME, &frame, |ctx| {
            // SAFETY: a context is current on this thread.
            unsafe { glow::HasContext::finish(ctx.raw_gl()) }
        });
        report(outcome(FRAME, timed));
    }
}

fn millis(duration: Duration) -> String {
    format!("{:.3} ms", duration.as_secs_f64() * 1000.0)
}

/// Why an unoptimized build is refused rather than measured.
///
/// `cargo xtask bench` runs through an alias that does not pass `--release`,
/// so for as long as this printed numbers it printed them for unoptimized
/// code. That is not a smaller version of the right answer, it is a different
/// one, and the difference falls almost entirely on one of the two paths being
/// compared: the analytic route submits a hundred and sixty draws where the
/// tessellated route submits one, so debug-build per-draw cost lands on the
/// analytic side and nowhere else.
///
/// It went wrong exactly that way. A commit that added a public method nothing
/// in the benchmark calls moved the analytic path from 13.16 ms to 13.82 ms on
/// a Raspberry Pi 5 -- reproducibly, on both Vulkan and GLES, with the
/// tessellated paths unmoved -- and deleting that uncalled method put it back.
/// A function nobody calls cannot cost GPU time; what it can do is shift code
/// layout in a build with no optimizer to absorb it. Built with `--release`
/// the same commit measures level with the tessellated path, which is what
/// this document's board figures say and what the design rests on.
///
/// `debug_assertions` is the test because the workspace builds under one
/// profile: if this binary is unoptimized then so is the renderer it times.
pub fn why_not_debug() -> &'static str {
    "bench: this is an unoptimized build, and timing one is worse than not \n\
     timing at all -- the cost falls on whichever path submits more draws, \n\
     which is the comparison this is for.\n\
     \n\
     Run it optimized:\n\
     \n\
     \x20   cargo run --release --package xtask -- bench\n\
     \n\
     For a board, cross-build with --release and copy that binary across.\n"
}

/// What a run that measured nothing says.
const NOTHING: &str = "no device on this machine could render the frame\n";

fn header() -> String {
    format!(
        "{SHAPES} rounded rectangles at {}x{}, then one frame of mixed \
         content at the same size.\n{FRAMES} frames each after {WARMUP} \
         warm-up\n",
        EXTENT.width, EXTENT.height
    )
}

/// What a run emits as it goes.
///
/// A device is announced when it is *opened* rather than when its first result
/// arrives, and that distinction is the reason this is an enum rather than a
/// stream of timings. A run on a Raspberry Pi 5 measured all three of V3D's
/// configurations and then took the board off the network before printing
/// anything about the next device -- and from the output alone there was no
/// way to tell whether it had died opening that device or measuring on it,
/// because both look like silence.
pub enum Event {
    /// A device was opened and its configurations are about to be measured.
    Device(String),
    /// One configuration finished.
    Measured(Timing),
    /// A configuration was attempted and could not be measured.
    ///
    /// Reported rather than dropped. `time_frames` used to answer `None` for
    /// every reason it could fail, and the caller had nowhere to put that, so
    /// a configuration the device refused simply did not appear -- and a row
    /// that is absent reads exactly like a row nobody asked for.
    ///
    /// A row *was* missing while this was written, and the cause was an
    /// unwritten call rather than a refusal. That is the argument rather than
    /// against it: the two are indistinguishable in a report that can only
    /// show what succeeded, and one of them was mistaken for the other for
    /// twenty minutes.
    Failed { what: &'static str, why: String },
    /// A device was opened and then not measured, because the caller asked for
    /// it to be left alone.
    ///
    /// Reported rather than passed over quietly. A run that measured two
    /// devices where the reader expected three, and said nothing about the
    /// third, is a run whose coverage cannot be read off its own output.
    Skipped(String),
}

/// One event's contribution to the report.
///
/// Both the streamed run and the assembled one go through here, so the format
/// cannot change in one and not the other.
fn render(event: &Event) -> String {
    let timing = match event {
        Event::Device(device) => return format!("\n{device}\n"),
        Event::Skipped(device) => return format!("\n{device}\n  not measured, as asked\n"),
        Event::Failed { what, why } => return format!("  {what:<24} not measured: {why}\n"),
        Event::Measured(timing) => timing,
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {:<24} {:>10} ({:>6.0} fps)   p99 {:>10}   fastest {:>10}   \
         slowest {:>10}   {:>4} draws{}\n",
        timing.what,
        millis(timing.median),
        timing.rate(),
        millis(timing.p99),
        millis(timing.fastest),
        millis(timing.slowest),
        timing.draws,
        if timing.noisy() { "   (noisy)" } else { "" }
    ));
    out
}

fn epilogue() -> &'static str {
    "\nEach device's lines are two comparisons and a budget. The first three \n\
     are one shape filled two ways and the next two are the same shape \n\
     stroked two ways, so within a group the difference is the route and not \n\
     the content -- and a fill's cost is not an outline's, which is why they \n\
     are not read across. The last line is not part of either. That is a whole \n\
     frame of mixed content -- a tabulated gradient behind, shadowed cards \n\
     over it, a blurred layer on top -- and it is there because a renderer can \n\
     be quick at a hundred and sixty identical rectangles and slow at \n\
     everything an interface is made of.\n\
     \n\
     None of these times the *recording*, which is where the stroke rules \n\
     live: a width widened to a pixel, the alpha that pays for it, and a \n\
     hairline snapped onto one all happen while the frame is being built, and \n\
     the clock starts after that. What the two stroked rows measure is what an \n\
     outline costs to draw -- the field's fragment work, and the geometry the \n\
     stroker emits, which is two contours where a fill has one.\n\
     \n\
     Medians, with the frame ninety-nine hundredths came in under beside \n\
     them. Nothing here passes or fails: these are what this machine did, \n\
     and the balance between the two paths is hardware-dependent by design \n\
     -- see the distance-field section of docs/architecture.md.\n\
     \n\
     The rate is what a median frame would sustain with nothing else in it: \n\
     no present, no vertical blank, and a scene that is a hundred and sixty \n\
     rectangles rather than an interface. Read it against the other paths \n\
     here rather than against a target in `plan.md`, which names a different \n\
     scene and counts a whole frame.\n\
     \n\
     Read each p99 against the median on its own line before reading it as \n\
     a renderer's tail. On a machine with a desktop on it a configuration \n\
     can come back with a median under three milliseconds and a ninety-ninth \n\
     percentile near thirty -- a tail ten times the frame it is the tail of, \n\
     which is this process being descheduled rather than the frame taking \n\
     that long. Which lines do it changes from run to run, and they are the \n\
     ones marked noisy. A p99 worth trusting wants a quiet runner, which is \n\
     where the regression gating this deliberately is not belongs too.\n"
}

/// Render the events a run would have emitted.
///
/// Only the tests reach this, because only they have events without a device
/// to measure them on; a real run takes [`stream`]. Both go through `render`,
/// so what the assertions below hold over is what a run prints rather than a
/// second rendering that happens to agree today.
#[cfg(test)]
pub fn text(events: &[Event]) -> String {
    if !events.iter().any(|e| matches!(e, Event::Measured(_))) {
        return NOTHING.to_string();
    }
    let mut out = header();
    for event in events {
        out.push_str(&render(event));
    }
    out.push_str(epilogue());
    out
}

/// Measure every configuration, writing each line as it is measured.
///
/// The header goes out before the first device is opened, so a run that is
/// killed before anything finishes still says what it was attempting -- and
/// every line is flushed, because a buffer that is never drained is the same
/// as not having printed at all.
/// Measure everything, write the report, and hand back what was measured.
///
/// The rows come back so a caller can record or check them. The report is
/// written either way -- a comparison is something done *with* the numbers,
/// never instead of printing them.
pub fn stream_collecting(
    skip: &[String],
    out: &mut impl Write,
) -> io::Result<Vec<(String, String, f64)>> {
    write!(out, "{}", header())?;
    out.flush()?;

    let mut rows: Vec<(String, String, f64)> = Vec::new();
    let mut device = String::new();
    let mut measured = 0usize;
    let mut failed = None;
    gather(skip, &mut |event| {
        if failed.is_some() {
            return;
        }
        match &event {
            Event::Device(name) => device.clone_from(name),
            Event::Measured(timing) => {
                measured += 1;
                rows.push((
                    device.clone(),
                    timing.what.to_string(),
                    timing.median.as_secs_f64() * 1000.0,
                ));
            }
            _ => {}
        }
        let line = render(&event);
        if let Err(e) = write!(out, "{line}").and_then(|()| out.flush()) {
            failed = Some(e);
        }
    });
    if let Some(e) = failed {
        return Err(e);
    }

    write!(out, "{}", if measured == 0 { NOTHING } else { epilogue() })?;
    out.flush()?;
    Ok(rows)
}

/// A recorded run, to compare a later one against.
///
/// Keyed by device *and* configuration, because a baseline from one device says
/// nothing about another: the whole point of the report naming its device is
/// that these numbers are not portable. A file recorded on a Pi and checked on
/// a workstation compares nothing, and this says so rather than passing.
///
/// Stored as text, one row per line, because a baseline that cannot be read in
/// a diff is a baseline nobody will question when it changes.
pub struct Baseline {
    rows: Vec<Row>,
}

/// One recorded configuration, and how much it is allowed to drift.
///
/// `slack` is a per-row override in fractional terms, and it exists because a
/// single tolerance has to be as loose as the *worst* row it covers. On a
/// Raspberry Pi 5 that is one row out of eight -- the Vulkan distance-field
/// figure, which lands in one of two states three percent apart -- while the
/// other seven repeat to within three tenths of a percent. Covering all eight
/// with one number meant a three percent regression on any of the seven passed
/// unremarked. A row that needs slack now says so itself, and says it next to
/// the measurement that justifies it.
struct Row {
    device: String,
    what: String,
    ms: f64,
    slack: Option<f64>,
}

impl Baseline {
    /// `device<TAB>what<TAB>median_ms` with an optional fourth field, a
    /// per-row tolerance in percent. Lines that are neither are an error
    /// rather than a skip -- a baseline half-read would gate on half the
    /// configurations and say nothing about the rest.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut rows = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split('\t');
            let (Some(device), Some(what), Some(ms), slack, None) = (
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
            ) else {
                return Err(format!(
                    "line {}: expected three tab-separated fields, or four with a tolerance",
                    number + 1
                ));
            };
            let ms: f64 = ms.trim().parse().map_err(|_| {
                format!(
                    "line {}: {ms:?} is not a number of milliseconds",
                    number + 1
                )
            })?;
            let slack = match slack {
                None => None,
                Some(text) => {
                    let percent: f64 = text.trim().parse().map_err(|_| {
                        format!(
                            "line {}: {text:?} is not a tolerance in percent",
                            number + 1
                        )
                    })?;
                    if !(percent.is_finite() && percent >= 0.0) {
                        return Err(format!(
                            "line {}: a tolerance of {percent} is not a fraction of a measurement",
                            number + 1
                        ));
                    }
                    Some(percent / 100.0)
                }
            };
            rows.push(Row {
                device: device.to_string(),
                what: what.to_string(),
                ms,
                slack,
            });
        }
        Ok(Self { rows })
    }

    pub fn render(&self) -> String {
        let mut out = String::from(
            "# cargo xtask bench --record\n\
             # device\tconfiguration\tmedian in milliseconds\t[tolerance %]\n",
        );
        for row in &self.rows {
            out.push_str(&format!("{}\t{}\t{:.3}", row.device, row.what, row.ms));
            if let Some(slack) = row.slack {
                out.push_str(&format!("\t{:.1}", slack * 100.0));
            }
            out.push('\n');
        }
        out
    }

    pub fn from_run(rows: &[(String, String, f64)]) -> Self {
        Self {
            rows: rows
                .iter()
                .map(|(device, what, ms)| Row {
                    device: device.clone(),
                    what: what.clone(),
                    ms: *ms,
                    slack: None,
                })
                .collect(),
        }
    }

    /// Carry each row's tolerance over from an earlier baseline.
    ///
    /// `--record` overwrites a file that a person edited: the per-row slack is
    /// a judgment about how repeatable a configuration is, not a measurement,
    /// and re-recording must not silently drop it. Dropping it would tighten
    /// the gate on exactly the row known to be unrepeatable, so the next run
    /// would fail for the reason the annotation existed to excuse.
    pub fn keeping_slack_from(mut self, previous: &Baseline) -> Self {
        for row in &mut self.rows {
            row.slack = previous
                .rows
                .iter()
                .find(|p| p.device == row.device && p.what == row.what)
                .and_then(|p| p.slack);
        }
        self
    }

    fn find(&self, device: &str, what: &str) -> Option<&Row> {
        self.rows
            .iter()
            .find(|r| r.device == device && r.what == what)
    }
}

/// What a comparison found, per row and in total.
pub struct Comparison {
    pub lines: Vec<String>,
    pub regressed: usize,
    /// Rows on one side and not the other. Counted rather than ignored: a
    /// baseline recorded on a machine with two devices and checked on one with
    /// one has not been satisfied, it has been half-read.
    pub unmatched: usize,
}

/// Compare a run against a baseline, tolerating `tolerance` as a fraction.
///
/// The tolerance is a fraction rather than a fixed number of milliseconds
/// because these span one millisecond to thirty depending on the device, and a
/// budget that means something on one would be noise or a hair trigger on the
/// other.
pub fn compare(baseline: &Baseline, run: &[(String, String, f64)], tolerance: f64) -> Comparison {
    let mut lines = Vec::new();
    let mut regressed = 0;
    let mut unmatched = 0;

    for (device, what, now) in run {
        match baseline.find(device, what) {
            Some(row) => {
                let then = row.ms;
                let allowed = row.slack.unwrap_or(tolerance);
                let delta = (now - then) / then;
                let over = delta > allowed;
                if over {
                    regressed += 1;
                }
                lines.push(format!(
                    "  {:<26} {:>9.3} was {:>9.3}  {:+6.1}%{}{}",
                    what,
                    now,
                    then,
                    delta * 100.0,
                    // Only where it differs from the default, so a reader sees
                    // at a glance which rows are being held to a looser bar.
                    match row.slack {
                        Some(s) => format!("  (tolerating {:.1}%)", s * 100.0),
                        None => String::new(),
                    },
                    if over { "   REGRESSED" } else { "" }
                ));
            }
            None => {
                unmatched += 1;
                lines.push(format!(
                    "  {what:<26} {now:>9.3}  no baseline for this configuration on {device}"
                ));
            }
        }
    }
    for row in &baseline.rows {
        let (device, what, then) = (&row.device, &row.what, row.ms);
        if !run.iter().any(|(d, w, _)| d == device && w == what) {
            unmatched += 1;
            lines.push(format!(
                "  {what:<26} {:>9}  was {then:>9.3} in the baseline for {device}, \
                 not measured now",
                "-"
            ));
        }
    }
    Comparison {
        lines,
        regressed,
        unmatched,
    }
}

/// The commit the recorded timings were last checked against, if a baseline
/// says so.
///
/// One file rather than all of them: a second board would need its own line and
/// its own count, and there is one board.
fn verified_at() -> Option<String> {
    let text = std::fs::read_to_string("tests/bench-baselines/raspberry-pi-5-v3d.txt").ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("# Last checked against the board: "))
        .map(|sha| sha.trim().to_string())
        .filter(|sha| sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()))
}

/// How far the recorded timings have drifted from the code, in commits.
///
/// The gate cannot check timing: it takes a quiet machine, and the one this
/// runs on spreads its own medians by up to half. So the timing baseline is
/// checked by hand on a board, and the failure that follows from that is not a
/// wrong number but a forgotten one -- sixteen renderer commits once went by
/// between two checks, and the ten and a half per cent they had cost was
/// invisible from every diff and every green gate.
///
/// This is the cheapest thing that would have surfaced it: not a threshold, not
/// a failure, just the count, printed where the skip census is printed and read
/// the same way. What it counts is commits touching the crates the bench times,
/// since the baseline file last changed.
///
/// `None` where the question cannot be asked -- no git, no baseline, a
/// checkout without history. A tarball build is not a build that has drifted.
pub fn baseline_drift() -> Option<(usize, &'static str)> {
    const BASELINE: &str = "tests/bench-baselines";
    /// What the bench times, end to end through both backends.
    const TIMED: &[&str] = &[
        "crates/impeller-core/src",
        "crates/impeller-shaders/shaders",
        "crates/impeller-hal/src",
        "crates/impeller-hal-vulkan/src",
        "crates/impeller-hal-gles/src",
    ];

    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git").args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    // The commit a board run last passed against, which the baseline records
    // itself. Falling back to when the file last changed would count the
    // commits since the numbers were *written*, and a check that passed without
    // re-recording -- which is the ordinary outcome -- would not be counted at
    // all. "I ran it" has to be a fact in the tree or it is not a fact.
    let recorded = match verified_at() {
        Some(sha) => sha,
        None => git(&["log", "-1", "--format=%H", "--", BASELINE])?,
    };
    if recorded.is_empty() {
        return None;
    }
    let mut args = vec!["log", "--oneline", &format!("{recorded}..HEAD"), "--"]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    args.extend(TIMED.iter().map(|p| p.to_string()));
    let listed = git(&args.iter().map(String::as_str).collect::<Vec<_>>())?;
    let count = listed.lines().filter(|l| !l.is_empty()).count();
    Some((count, BASELINE))
}

/// The line the gate prints for [`baseline_drift`].
pub fn drift_line() -> Option<String> {
    let (count, path) = baseline_drift()?;
    Some(match count {
        0 => format!("the timing baseline in {path} is current\n"),
        1 => format!(
            "1 commit has touched what the bench times since {path} was \
             recorded. Timing is not gated here; `cargo xtask bench --check` on \
             a board is what would say.\n"
        ),
        n => format!(
            "{n} commits have touched what the bench times since {path} was \
             recorded. Timing is not gated here; `cargo xtask bench --check` on \
             a board is what would say.\n"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A timing with unremarkable numbers, for the tests that are about the
    /// report rather than about what was measured.
    fn a_timing(what: &'static str) -> Timing {
        Timing {
            what,
            draws: SHAPES,
            median: Duration::from_micros(100),
            p99: Duration::from_micros(140),
            fastest: Duration::from_micros(90),
            slowest: Duration::from_micros(110),
        }
    }

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
        let rendered = text(&[Event::Measured(a_timing(Path::Analytic.name()))]);
        assert!(rendered.contains("160 draws"), "{rendered}");
    }

    #[test]
    fn a_stroked_row_strokes_and_takes_the_route_it_is_named_for() {
        // The two stroked rows exist to be a comparison, which they are only if
        // each takes the route its name claims. Read off the draw count, which
        // is what separates the two: the analytic field carries the shape's
        // parameters in its own material, so a hundred and sixty of them cannot
        // merge, and the tessellated one puts every outline through a single
        // solid material and comes out as one draw. A stroked row that had
        // quietly fallen back to tessellation would report one draw here and
        // measure the same thing twice.
        assert_eq!(
            recording(Path::StrokedAnalytic).draw_count(),
            SHAPES,
            "a stroked field is one draw per shape, so falling back to the \
             tessellator would show here"
        );
        assert_eq!(
            recording(Path::StrokedTessellated).draw_count(),
            1,
            "stroked outlines share one material and merge"
        );

        // And they stroke. A fill and an outline of the same rounded rectangle
        // differ in the geometry submitted, so the vertex counts cannot match --
        // an outline is two contours where a fill is one.
        let vertices = |path| recording(path).passes[0].batch.vertices().len();
        assert_ne!(
            vertices(Path::StrokedTessellated),
            vertices(Path::TessellatedSingleSampled),
            "a stroked path and a filled one submitted the same geometry, so one \
             of them is not doing what it says"
        );
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

    /// A device is named when it is opened, so a run that dies measuring on it
    /// still says which one it was on.
    ///
    /// This is the whole reason [`Event`] is an enum. When a device's name
    /// arrived with its first result, a run that opened a device and finished
    /// no configuration on it printed nothing about that device at all --
    /// which is what a run on a Raspberry Pi 5 did, leaving no way to tell
    /// whether it had died opening the second device or measuring on it.
    #[test]
    fn a_device_that_measures_nothing_is_still_named() {
        let rendered = text(&[
            Event::Device("vulkan:0 a".into()),
            Event::Measured(a_timing(Path::Analytic.name())),
            Event::Measured(a_timing(Path::TessellatedMultisampled.name())),
            // Opened, and then the run ended before a configuration finished.
            Event::Device("vulkan:1 b".into()),
        ]);

        assert!(
            rendered.contains("vulkan:1 b"),
            "a device that measured nothing went unnamed: {rendered}"
        );
        assert_eq!(rendered.matches("vulkan:0 a").count(), 1, "{rendered}");
        assert_eq!(rendered.lines().filter(|l| l.starts_with("  ")).count(), 2);
    }

    /// A device is matched by any part of the name it gives itself.
    ///
    /// The name carries a driver version and a vector width, neither of which
    /// a caller can be expected to type, so matching the whole of it would
    /// make the option unusable and silently measure the device anyway.
    #[test]
    fn a_device_is_skipped_on_any_part_of_the_name_it_gives_itself() {
        let name = "vulkan:1 llvmpipe (LLVM 22.1.8, 256 bits)";
        assert!(skipped(name, &["llvmpipe".into()]));
        assert!(
            skipped(name, &["LLVMpipe".into()]),
            "case should not matter"
        );
        assert!(skipped(name, &["vulkan:1".into()]));
        assert!(!skipped(name, &["v3d".into()]));
        assert!(!skipped(name, &[]), "nothing named, nothing skipped");

        // And the hardware device beside it is not caught by the same pattern,
        // which is the whole point of asking for one.
        assert!(!skipped("vulkan:0 V3D 7.1.7.0", &["llvmpipe".into()]));
    }

    /// A device left unmeasured says so where its results would have been.
    #[test]
    fn a_skipped_device_is_named_rather_than_passed_over_quietly() {
        let rendered = text(&[
            Event::Device("vulkan:0 V3D 7.1.7.0".into()),
            Event::Measured(a_timing(Path::Analytic.name())),
            Event::Skipped("vulkan:1 llvmpipe".into()),
        ]);
        assert!(rendered.contains("vulkan:1 llvmpipe"), "{rendered}");
        assert!(rendered.contains("not measured"), "{rendered}");
    }

    /// The refusal names the command to run instead.
    ///
    /// A refusal that says only "no" costs the reader the twenty minutes it
    /// took to work out what to do about it, and this one is reached by
    /// someone who typed the documented invocation and got nothing.
    #[test]
    fn refusing_an_unoptimized_build_says_what_to_run_instead() {
        let said = why_not_debug();
        assert!(said.contains("--release"), "{said}");
        assert!(said.contains("cargo run"), "{said}");
        assert!(said.contains("bench"), "{said}");
        // And why, because a reader who does not believe it will run it anyway.
        assert!(said.contains("draws"), "{said}");
    }

    /// The frame reaches the machinery the three-route comparison never does.
    ///
    /// Without this the frame is a slower way to measure what is already
    /// measured. Each assertion names one thing the comparison cannot reach:
    /// a second pass is a layer, having its own target and a composite back;
    /// a tabulated ramp is a gradient past `MAX_STOPS`, which is an upload and
    /// a sampled texture rather than four colors riding inside a material.
    #[test]
    fn the_full_frame_reaches_a_layer_and_a_ramp() {
        let frame = full_frame();
        assert!(
            frame.passes.len() > 1,
            "no layer in the frame: {} pass(es)",
            frame.passes.len()
        );
        assert!(
            !frame.ramps.is_empty(),
            "no tabulated gradient, so the ramp is untimed"
        );
        // And it is a frame rather than one shape: ground, three cards, three
        // shadows and a highlight.
        assert!(frame.draw_count() >= 5, "{} draws", frame.draw_count());
    }

    /// A row's own tolerance is what gates it, not the default.
    ///
    /// The mutation that must break this: delete the `slack.unwrap_or` in
    /// `compare` and both halves fail at once -- the marked row starts failing
    /// on drift it is known to have, and the unmarked one stops failing on
    /// drift it does not.
    #[test]
    fn a_row_carrying_a_tolerance_is_held_to_that_one() {
        let baseline = Baseline::parse(
            "dev\tbimodal\t10.0\t5.0\n\
             dev\tsteady\t10.0\n",
        )
        .expect("should parse");

        // Three percent: inside the marked row's five, outside the default one.
        let found = compare(
            &baseline,
            &rows(&[("dev", "bimodal", 10.3), ("dev", "steady", 10.3)]),
            0.01,
        );
        assert_eq!(found.regressed, 1, "{}", found.lines.join("\n"));
        assert!(
            found
                .lines
                .iter()
                .any(|l| l.contains("steady") && l.contains("REGRESSED")),
            "the unmarked row is the one that should fail: {}",
            found.lines.join("\n")
        );

        // And the marked row is not exempt, only looser: six percent fails it.
        let found = compare(&baseline, &rows(&[("dev", "bimodal", 10.6)]), 0.01);
        assert_eq!(found.regressed, 1, "{}", found.lines.join("\n"));
    }

    /// A tolerance survives the re-record that overwrites the file.
    ///
    /// This is the failure the annotation would otherwise cause: `--record`
    /// writes fresh numbers over a file a person edited, and if the slack went
    /// with them the next check would gate the one row known to be
    /// unrepeatable at the tight default and fail for the reason the
    /// annotation existed to prevent.
    #[test]
    fn re_recording_keeps_the_tolerances_it_overwrites() {
        let previous =
            Baseline::parse("dev\tbimodal\t10.0\t5.0\ndev\tsteady\t10.0\n").expect("should parse");
        let text = Baseline::from_run(&rows(&[("dev", "bimodal", 11.0), ("dev", "steady", 11.0)]))
            .keeping_slack_from(&previous)
            .render();

        assert!(text.contains("bimodal\t11.000\t5.0"), "{text}");
        assert!(text.contains("steady\t11.000\n"), "{text}");

        // And it round-trips, so the next re-record keeps it too.
        let read = Baseline::parse(&text).expect("its own output should parse");
        let found = compare(&read, &rows(&[("dev", "bimodal", 11.3)]), 0.01);
        assert_eq!(found.regressed, 0, "{}", found.lines.join("\n"));
    }

    /// A fourth field that is not a tolerance is an error, not a shrug.
    #[test]
    fn a_tolerance_that_is_not_one_is_refused() {
        assert!(Baseline::parse("d\tw\t1.0\tloose").is_err());
        assert!(Baseline::parse("d\tw\t1.0\t-2.0").is_err());
        assert!(Baseline::parse("d\tw\t1.0\t2.0\textra").is_err());
        assert!(
            Baseline::parse("d\tw\t1.0\t0").is_ok(),
            "zero slack is a choice"
        );
    }

    fn rows(v: &[(&str, &str, f64)]) -> Vec<(String, String, f64)> {
        v.iter()
            .map(|(d, w, m)| (d.to_string(), w.to_string(), *m))
            .collect()
    }

    /// A baseline survives being written and read back.
    #[test]
    fn a_baseline_round_trips_through_its_own_format() {
        let original = rows(&[
            ("vulkan:0 V3D 7.1.7.0", "distance field, 1 sample", 13.376),
            ("gles V3D 7.1.7.0", "full frame, mixed content", 21.209),
        ]);
        let text = Baseline::from_run(&original).render();
        let read = Baseline::parse(&text).expect("its own output should parse");
        let found = compare(&read, &original, 0.0);
        assert_eq!(found.regressed, 0, "{:?}", found.lines);
        assert_eq!(found.unmatched, 0, "{:?}", found.lines);
    }

    /// A line that is neither three fields nor four stops the read rather
    /// than being skipped: a baseline half-understood gates on some
    /// configurations and silently ignores the others.
    #[test]
    fn a_malformed_baseline_is_refused_rather_than_partly_read() {
        assert!(Baseline::parse("device\tconfiguration").is_err());
        assert!(Baseline::parse("device\tconfiguration\tnot-a-number").is_err());
        // Comments and blank lines are not malformed.
        let ok = Baseline::parse("# a note\n\ndev\twhat\t1.5\n").expect("should parse");
        assert_eq!(ok.find("dev", "what").map(|r| r.ms), Some(1.5));
    }

    /// The device is part of the key, so a baseline from another machine does
    /// not quietly pass.
    #[test]
    fn a_baseline_from_another_device_matches_nothing() {
        let recorded = Baseline::from_run(&rows(&[("pi", "distance field", 13.0)]));
        let found = compare(
            &recorded,
            &rows(&[("workstation", "distance field", 0.9)]),
            0.05,
        );
        assert_eq!(found.regressed, 0, "a different device is not a regression");
        assert_eq!(
            found.unmatched, 2,
            "both rows are unmatched: {:?}",
            found.lines
        );
    }

    /// Slower past the tolerance fails; slower within it, and faster, do not.
    #[test]
    fn only_a_regression_past_the_tolerance_counts() {
        let recorded = Baseline::from_run(&rows(&[("d", "w", 10.0)]));
        let at = |now: f64| compare(&recorded, &rows(&[("d", "w", now)]), 0.05).regressed;
        assert_eq!(at(10.4), 0, "four percent slower is inside five");
        assert_eq!(at(10.6), 1, "six percent slower is not");
        assert_eq!(at(5.0), 0, "faster is never a regression");
        assert_eq!(at(10.5), 0, "exactly the tolerance is inside it");
    }

    #[test]
    fn a_wide_spread_is_reported_rather_than_hidden() {
        let timing = |fastest: u64, slowest: u64| Timing {
            what: "test",
            draws: SHAPES,
            median: Duration::from_micros((fastest + slowest) / 2),
            p99: Duration::from_micros(slowest),
            fastest: Duration::from_micros(fastest),
            slowest: Duration::from_micros(slowest),
        };
        assert!(timing(100, 500).noisy());
        assert!(!timing(100, 150).noisy());
        assert!(text(&[Event::Measured(timing(100, 500))]).contains("noisy"));
    }
}
