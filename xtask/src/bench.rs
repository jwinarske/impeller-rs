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
    "\nThe first three lines of each device are one comparison: the same \n\
     shapes drawn two ways, so the difference between them is the route and \n\
     not the content. The last line is not part of it. That is a whole frame \n\
     of mixed content -- a tabulated gradient behind, shadowed cards over it, \n\
     a blurred layer on top -- and it is there because a renderer can be \n\
     quick at a hundred and sixty identical rectangles and slow at \n\
     everything an interface is made of. Read it against a frame budget; \n\
     read the three above it against each other.\n\
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
pub fn stream(skip: &[String], out: &mut impl Write) -> io::Result<()> {
    write!(out, "{}", header())?;
    out.flush()?;

    let mut measured = 0usize;
    let mut failed = None;
    gather(skip, &mut |event| {
        if failed.is_some() {
            return;
        }
        if matches!(event, Event::Measured(_)) {
            measured += 1;
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
    out.flush()
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
