# Running the suite on a board

A workstation cannot answer the question this suite is mostly built to ask.
`conformance` renders every corpus scene on two devices and requires them to
agree, which is the check that scales: adding a scene extends coverage across
every device the corpus runs on without anyone certifying a reference image.
On a machine whose only Vulkan devices are llvmpipe and a software reference,
those two devices are the same kind of thing, and a difference that only a
real driver produces cannot appear.

A Raspberry Pi 5 has both halves: v3d, which is a tiler with a real shader
compiler, and llvmpipe beside it. Pointing `conformance` at that pair is what
the following is for.

## Cross-building without a target libc

Fedora's `gcc-aarch64-linux-gnu` installs a compiler and leaves
`/usr/aarch64-linux-gnu/sys-root` empty, so compilation succeeds and linking
fails on everything at once — `cannot find -lc`, `cannot find crtn.o`. That
reads as a broken toolchain and is not one; there is simply nothing to link
against.

Take the sysroot from the board. It runs the exact glibc, gcc runtime and
kernel headers the binary will meet, which makes it a better sysroot than a
packaged one and a worse thing to forget you depend on. About twenty-seven
megabytes, once.

```sh
S=$HOME/.cache/pi-sysroot
PI=joel@raspberrypi

mkdir -p "$S/usr/lib/aarch64-linux-gnu" "$S/usr/lib/gcc"
rsync -a \
  --include='*.o' --include='*.a' \
  --include='libc.so*' --include='libm.so*' --include='libmvec.so*' \
  --include='libdl.so*' --include='libpthread.so*' --include='librt.so*' \
  --include='libutil.so*' --include='libanl.so*' --include='libgcc_s.so*' \
  --include='ld-linux-aarch64*' --exclude='*' \
  "$PI:/usr/lib/aarch64-linux-gnu/" "$S/usr/lib/aarch64-linux-gnu/"
rsync -a "$PI:/usr/lib/gcc/aarch64-linux-gnu" "$S/usr/lib/gcc/"

ln -sfn usr/lib "$S/lib"
ln -sfn aarch64-linux-gnu/ld-linux-aarch64.so.1 "$S/usr/lib/ld-linux-aarch64.so.1"
```

Include `*.o` rather than `crt*.o`: the glob is case-sensitive, and `Scrt1.o` —
the start file for a position-independent executable — is what Rust asks for.

```sh
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="\
-C link-arg=--sysroot=$S \
-C link-arg=-B$S/usr/lib/aarch64-linux-gnu \
-C link-arg=-L$S/usr/lib/aarch64-linux-gnu \
-C link-arg=-L$S/usr/lib/gcc/aarch64-linux-gnu/14"

cargo test --workspace --exclude xtask --target aarch64-unknown-linux-gnu --no-run
```

All four link arguments earn their place, and each is missed differently:
without `--sysroot` the linker scripts resolve `/lib/...` against the host;
without `-B` the start files are not found; without the multiarch `-L`, `-lc`
is not found; without the gcc `-L`, `-lgcc_s` is not found. The `14` is gcc's
major version on the board — check it rather than assume.

Nothing above installs Vulkan, EGL, GBM or libdrm for the target, and nothing
needs to: `ash` opens the loader at runtime and `drm-rs` issues ioctls through
`rustix`, so neither reaches the linker.

## Running them there

`--no-run` prints each binary it built. Copy them across and run them; there is
no cargo on the board and none needed.

```sh
scp target/aarch64-unknown-linux-gnu/debug/deps/* "$PI:/tmp/pibins/"
ssh "$PI" 'cd /tmp/pibins && for b in *; do ./$b --test-threads=1; done'
```

Two things the harness needs, both of which look like failures when missing.

`--test-threads=1`, for anything taking DRM master: two tests racing for it
report the second as unable to open a card, which reads as a device problem and
is a scheduling one.

`IMPELLER_SHADER_SNAPSHOTS`, naming a directory holding `tests/shader-snapshots`
copied across. The snapshot tests find their files from `CARGO_MANIFEST_DIR`,
which is baked in at compile time and names a path on the machine that did the
compiling. Without it they are the only two tests here that cannot run from a
bare binary, and a board run can never come out clean.

`IMPELLER_COST_BASELINE`, naming a copy of
`crates/impeller-testkit/tests/cost-baseline.txt`, for the same reason and with
the same failure. The cost table is counted rather than measured, so a board is
where the claim that it is device-independent gets tested against a different
architecture instead of a different driver -- which is worth doing and cannot
be done from a binary that looks for its baseline on the machine that compiled
it. Recorded on x86-64, it matched byte for byte on the Pi 5.

`IMPELLER_DRM_CARD` matters on a board with more than one display controller. A
Pi 5 has two, and a test that opens the first `/dev/dri/cardN` gets `rp1-dsi`
rather than `vc4`. A suite that passes on the controller that works says nothing
about the one beside it; `impeller-present-drm`'s crate documentation has the
table of what each board actually does.

## Benchmarking there, which has two rules of its own

The suite above runs from a debug build and should. `cargo xtask bench` must
not, and refuses to: cross-build it with `--release` and copy that binary.

```sh
# with $S and the two CARGO_TARGET_* exports above already set
cargo build -p xtask --release --target aarch64-unknown-linux-gnu
scp target/aarch64-unknown-linux-gnu/release/xtask "$PI:/tmp/xtask"
ssh "$PI" 'chmod +x /tmp/xtask && /tmp/xtask bench --skip llvmpipe'
```

The comment is there because this block looks self-contained and is not. Run it
in a fresh shell and the link fails four times over, each time differently and
each time in a way that reads as a broken cross toolchain: `incompatible with
elf64-x86-64` while the host linker is still being used, then `cannot find -ldl`,
then `cannot find -lgcc_s`, then `cannot find Scrt1.o`. Every one of those is
answered by the paragraph under the export block above, which is worth reading
before improvising a fix for any of them.

The reason is not that debug is slower. The benchmark compares a path that
submits a hundred and sixty draws against one that submits a single merged
draw, so unoptimized per-draw cost lands on one side of the comparison and not
the other. On this board that is the difference between a real number and a
five percent regression that is not there — see the distance-field section of
[`architecture.md`](architecture.md), which records how one was chased. The two
binaries are easy to tell apart when you are not sure which got copied: the
release one is about 1.7 MB and the debug one about 30 MB.

`--skip llvmpipe` leaves the software rasterizer unstarted. Measuring on it
holds all four of the Pi 5's cores flat out for minutes at 1920x1080, and the
board locked up at that stage four times in a row, each time having come
through V3D's configurations first — once leaving the DSI panel full white with
the network gone, which looks more like a kernel or display hang than a supply
that cannot hold up. Whether it is power or heat is not established: throttle
flags read clean beforehand and two of the four were from a cold boot. The
board's own GPU is the number worth having and it is measurable without ever
starting that stage.

`/tmp` is wiped on reboot, so a lockup costs the binary as well as the run.
A bench that dies instantly with `nohup: failed to run command './xtask'` is
that, not the board.

## What every fragment was paying for draws that never asked

Three fixes of one kind, found by pulling on a single unexplained row and
measured on the Pi 5 with three runs a side agreeing to a hundredth of a
millisecond. Each is work every fragment did that its own draw had not asked
for.

`shade` dispatched on the material kind with a chain of `if`s, each testing an
upper and a lower bound, so a fragment paid two float comparisons for every kind
ahead of its own -- and a solid color, kind zero and the commonest material
there is, matched none of the fourteen and fell through all of them to reach the
return at the bottom. It is a `switch` now with solid answered first.

`fs_main` called `blend_tint` unconditionally, to combine a vertex color with
the material. That function carries twenty-nine modes and the non-separable tail
behind them, and the mode a draw that asked for nothing gets is `Modulate` --
which is `src * dst`, one multiply. Written out at the call site, so the common
case does not enter the function at all.

`dithered` computed which kinds are dithered -- four comparisons -- before the
amplitude test that discards the answer for every draw that is not a gradient's
into a quantized target. The amplitude is tested first now.

| row | 2026-08-26 | now |
|---|---|---|
| Vulkan distance field, 1 sample | 13.363 | 9.225 |
| Vulkan tessellated, 4 samples | 15.976 | 4.523 |
| Vulkan tessellated, 1 sample | 13.661 | 3.694 |
| Vulkan full frame, mixed content | 21.199 | 14.623 |
| GLES distance field, 1 sample | 13.442 | 8.714 |
| GLES tessellated, 4 samples | 15.971 | 5.459 |
| GLES tessellated, 1 sample | 13.530 | 3.653 |
| GLES full frame, mixed content | 20.397 | 15.033 |

**Hash every scene before and after, and do not skip it.** No test in this tree
would have caught a mistake in any of the three: both backends run the same
WGSL, and the "software reference" the corpus compares against is lavapipe
running it too, so a shared error passes every comparison the suite makes. What
was done instead was to hash the pixels of all four hundred and twenty-six
catalog and corpus scenes on each side of each change. Zero differed, twice, and
that is what separates a dispatch cost from a shortcut.

**One wrong turn is worth keeping.** The first attempt at the dispatch reordered
the chain rather than replacing it: it recovered the rows it was aimed at and
made the mixed frame three per cent *worse*, because moving one kind up moves
every kind below it down. A switch removes that property rather than rebalancing
it.

The general lesson is about where this was visible from, which is nowhere except
the board. Not from a diff -- all three had been correct and unremarkable for as
long as they existed. Not from `cost.rs`, which counts passes, draws and
vertices and was right about all three. Not from the catalog, whose pixels do
not move. The chain had been there since the shader had kinds to dispatch on,
and the numbers it cost had been recorded as the baseline and read as the cost
of the work.

## Advanced blending on this machine's Vulkan, twice

Two draws answer an advanced blend with an empty frame on lavapipe, and GLES --
the same Mesa through a different extension -- is correct for both. Neither is
this renderer: the pipeline state is the same either way and the pictures agree
on GLES.

The first is any advanced blend under multisampling. Fifteen catalog plates were
reporting a mode they never drew because of it, and they are single-sampled now,
which costs them nothing: their subject is what a blend computes rather than
where an edge falls.

The second is a *group* composited with an advanced mode, which is an image quad
drawn with that mode. It produces nothing at any alpha and any sample count,
where the same mode on an ordinary draw works. There is no workaround for it
here -- the composite is what a group is -- so the sweep that asks whether a
plate can show its mode asks every device and passes if any can.

Both were found by writing the plate and looking at the output, and neither is
visible from a diff, from `cost.rs`, or from the cross-backend comparison, which
skips these scenes because the *preferred* Vulkan device has no advanced-blend
extension at all.

## Saying when it was last checked

`cargo xtask gate` prints one line about the timing baseline, beside the skip
census and read the same way: how many commits have touched what the bench times
since a board run last passed. It is not a threshold and it cannot fail; timing
needs a quiet machine, and the one the gate runs on spreads its own medians by
up to half.

The count is from a line in the baseline itself -- `# Last checked against the
board:` and a commit -- rather than from when the file last changed. The
difference matters, because the ordinary outcome of a check is that the numbers
*pass* and are not re-recorded, and keying on the file would count those as
drift. Update the line when a `--check` run passes, whether or not you re-record.
A test asserts the line is there and parses, since nothing else would notice a
comment in a data file going missing, and a baseline that has stopped saying
when it was checked reports "current" forever.

### Which commit it names, and the way that went wrong

A hash cannot be recorded inside the object it names, so the line always names a
commit older than the one writing it -- and choosing which older one is where
this went wrong once, in a way worth keeping because the failure looked like a
correction.

The line had named the commit before the one whose numbers were recorded, so the
gate read one commit of drift where the answer was none. That was fixed by moving
the line forward one commit. The reasoning was right and the commit it landed on
was not: the commit it moved to *changes what the bench times*, and says so in
its own message -- "the distance-field benchmark came back four per cent faster
on the Pi 5". So the line came to name a state no board run had ever passed
against, and the drift count started from the wrong place.

Nothing could see it. A `--check` was not run at the moment the line moved,
because moving it was a documentation fix; and when one was run eight commits
later it failed, on a row that had nothing to do with any of those eight.

The rule that comes out of it: **the line may only name a commit that touches
nothing the bench times, or the commit whose state a run actually passed
against.** A commit that only edits the baseline file qualifies, so the ordinary
case -- re-record and name the commit you measured, in the same commit -- is
both legal and the shortest path. Moving the line onto a renderer commit is what
is not allowed, however plausible the arithmetic looks.

### What that hid

Finding it needed three commits benched, three runs each, in one sitting on one
board -- the commit the numbers came from, the commit the line had been moved
to, and the tip. The first reproduced the recorded numbers to within four tenths
of a percent and passed. The tip was level with the middle one everywhere, the
largest gap between them a fifth of a percent. So all of the movement belonged
to the middle commit, which drew a stroke as the difference of two offset shapes:

| row | before | after | |
|---|---|---|---|
| vulkan distance field | 9.225 | 8.857 | -4.0%, which its message claimed |
| gles distance field | 8.714 | 8.919 | +2.4%, which nobody noticed |
| vulkan full frame | 14.623 | 14.442 | -1.2% |
| gles full frame | 15.033 | 14.530 | -3.3% |

The four tessellated rows do not move at all, which is the check on that
attribution: the change is to the analytic stroke and those rows never take it.

The commit is a net win on this board and one row of it is a loss. Both
full-frame numbers improve, and a frame of mixed content is what an interface
pays; the GLES analytic-stroke row costs two and a half per cent for it. Worth
recording rather than averaging away, because the two backends run the same
WGSL: a change that moves them in opposite directions is a fact about the two
compilers, and the next such change wants this one to have been written down.

It went unseen because of what sits beside it. The Vulkan row it is paired with
carries a five per cent tolerance of its own, for a bimodality documented in the
baseline's header -- so the run that would have failed on GLES was the only one
that could have said anything, and it was never made.

### A function nothing called, compiled by every driver

The same measurement has a corollary nobody had drawn from it. A shader is not
a program with a linker that drops what nothing reaches: naga emits an uncalled
function into the GLSL exactly as it emits a used one, so dead code in a shader
source is code every driver parses and compiles for the life of the program.

`solid.wgsl` had one. `outline_if_asked` went in with the commit that traced an
outline from the same distance field, was never called by anything, and stayed
for every commit after -- two hundred and seven bytes of GLSL and seventy-eight
words of SPIR-V. Whether that is measurable is not established and this document
will not guess; what is established, in the section below, is that this shader's
cost is a step function of its size on this board, and that is enough reason not
to carry code nothing runs.

`every_function_in_a_shader_is_either_called_or_a_stage` in
`crates/impeller-shaders/tests/sources.rs` is what would have said so. It reads
the WGSL rather than the generated output, since a call graph is legible there
and not in the GLSL, and it catches a chain one link at a time: a helper called
only from a dead function still reads as called, so removing the root is what
names the next one.

### And a smaller shader is not a faster one

The corollary above says not to carry code nothing runs. It does not say that
removing code makes anything quicker, and the same board says plainly that it
does not.

Two commits shrank `solid.wgsl` in one afternoon. `1f64a28` folded a rounded
rectangle's fill and its outline into one expression, which took 431 bytes of
GLSL and 68 SPIR-V words out; `e9833c8` deleted the uncalled function above,
another 207 bytes and 78 words. Benched three commits over sixteen runs, four a
side in the faster of the two Vulkan states:

| row | before | after | |
|---|---|---|---|
| vulkan full frame | 14.444 | 13.955 | -3.4% |
| vulkan tessellated x4 | 4.481 | 4.522 | +0.9% |
| vulkan tessellated x1 | 3.661 | 3.691 | +0.8% |
| gles full frame | 14.540 | 14.835 | +2.0% |

All of it is `1f64a28`. The deletion measures at nothing at all -- four runs of
the tip against four of `1f64a28` alone are indistinguishable on every row --
which is the answer to the question that commit deliberately left open, and is
what should happen if a driver drops unreachable code from the program it
actually compiles even though naga emits it into the source.

Three things worth keeping from the rest of it. The change went *four* ways at
once, not one: a frame three per cent cheaper on one backend and two per cent
dearer on the other, from the same source. Neither distance-field row moved,
and that is the path the folded function serves -- so what the other four rows
responded to is not the arithmetic that changed but the size and shape of the
program around it. And the direction is not predictable from the size: a
strictly smaller shader made the row that matters most faster on Vulkan and
slower on GLES.

The lesson is the one above it, with the sign removed. Shader size is a step
function on this board, the steps are not all downhill, and the only way to know
which way one goes is to run it.

## What a shader costs on this board, measured the hard way

The baseline went sixteen renderer commits unchecked, and checking it found
every row between five and eleven per cent slower. The board had not changed:
the baseline commit, cross-built and run the same afternoon, reproduced its own
numbers to a tenth of a tenth of a per cent -- 13.362 against 13.363 -- which is
what makes the rest of this a statement about the code.

Bisected on the board, one cross-build and one run per step, reading the
tessellated single-sample row:

| commit | ms | against the baseline |
|---|---|---|
| the baseline | 13.661 | — |
| a nine-patch in one draw | 13.663 | +0.0% |
| a point field in one draw | 14.148 | +3.6% |
| a blend as a color filter | 14.713 | +7.7% |
| a gradient at a mesh's coordinates | 15.125 | +10.7% |

Three shader changes, each individually reasonable, each about three points.
None of them added work to the path being measured: a tessellated rectangle
filled with a solid color takes no gradient, no point field and no color filter.
What they added was *size* -- another material kind, another branch in the
filter tail, another coordinate -- to a shader every draw compiles.

One of the three was also a plain mistake, and fixing it recovered all of them.
`select` in this language evaluates both of its operands, so choosing between two
calls to the gradient mapping ran it twice per fragment; and it had been hoisted
above the branch chain, so a solid fill ran it twice as well. Selecting the
*input* and calling the mapping once, from inside a gradient's arm, put the
tessellated rows at 1.6 to 1.7 per cent *below* the baseline.

That the last three points of a ten-point regression were worth twelve is the
part to remember. The cost of a fragment shader here is not the sum of what its
branches do; it is a step function of what the whole thing needs at once, and
the compiler's register budget is the step. So a change that adds nothing to the
measured path can still cost three per cent, and a change that removes a little
can recover much more. Neither is visible from a diff, and neither is visible
from `cost.rs`, which counts passes, draws and vertices and is right about all
three.

**Measure through GLES on this board. One Vulkan configuration will not hold
still, and it is the one worth measuring.** Ten runs of a binary gave a GLES
distance-field figure spread over 0.017 ms and a Vulkan one that jumped between
13.35 and 13.82 — same process, same GPU, printed minutes apart in the same
run, and only one of them moving. So it is not heat, not the clock (V3D sat at
960 MHz throughout) and not the board.

Narrower than that: within one run the other two Vulkan configurations are
steadier than the GLES row. Over six runs the tessellated single-sampled figure
spread 0.008 ms and the four-sample one 0.05, while the distance field beside
them spread 0.277. What separates that configuration is draw count — an
analytic shape carries its geometry inside its material and cannot merge, so it
is a hundred and sixty draws where the tessellated ones are a single merged
draw. The variability is therefore in something the Vulkan backend does per
draw rather than per frame, and it does it differently from one process to the
next.

It scales with the draws, which is the next thing that was worth measuring
rather than assuming. Tripling the shape count from a hundred and sixty to four
hundred and eighty took the gap from 0.45 ms to 1.06 -- about two and a half
microseconds per draw either way. So the two states differ in what a draw
costs, not in a fixed charge per frame.

One candidate is ruled out and it is the obvious one. Every submission
allocates, fills and frees its buffers, and the analytic configuration's
materials buffer is forty kilobytes against the tessellated one's two hundred
and fifty-six — so host-visible bytes written looks like the culprit until the
geometry is counted. The analytic frame carries 640 vertices and 960 indices;
the tessellated frame carries 3840 and 10560. The *stable* configuration writes
several times more host-visible memory per frame than the unstable one, so the
quantity written is not what varies.

What is left is per-draw work: a hundred and sixty descriptor rebinds, pipeline
lookups and draw calls against one of each. Hashing is not enough to explain it
— two `HashMap` lookups a draw at tens of nanoseconds against a gap of two and
a half *micro*seconds a draw.

Memory placement is not it either, which was the next guess and is now ruled
out. Printing what `gpu-allocator` chose for a submission's first host-visible
buffer gave the same answer in every run — `DEVICE_LOCAL | HOST_VISIBLE |
HOST_COHERENT`, offset 8294656, size 23040, byte for byte — across runs that
came out fast and one that came out slow. Same memory type, same offset, same
size, different speed.

So: not the quantity written, not where it was written, not the hashing. What
remains is the driver's own cost of a draw — and which half of that, recording
or executing, turns out to be answerable without `perf`, which is not installed
here anyway. Timing the two phases separately inside the backend, around the
`vkCmd*` loop and around the submit-and-wait, separates them cleanly. Over
thirty-one runs, per frame at a hundred and sixty draws:

| | recording | submit and wait | frame |
|---|---|---|---|
| fast, 26 runs | 0.66 ms | 12.66 ms | 13.36 ms |
| slow, 5 runs | 1.01 ms | 12.70 ms | 13.81 ms |

**The gap is in recording commands, not in running them.** Recording separates
the two states by 57 percent; submit-and-wait, which contains all of the GPU's
work, differs by 0.3 and accounts for a twelfth of the gap. Every slow frame
had a slow recording phase and no fast frame did — the correlation is exact
across all thirty-one. The extra 0.35 ms over a hundred and sixty draws is 2.2
microseconds each, which is the same per-draw figure the shape-count sweep
above arrived at from the outside.

The GLES device is the control and reads zero on both counters, since they
count only what the Vulkan backend does.

Two more per-process candidates are ruled out, both of them things fixed when a
process starts. Pinned to one core with `taskset -c 2`, one run in ten still
came out slow; with address-space randomization off under `setarch -R`, two in
ten did. So it is neither which core the recording runs on nor where the
driver's code and buffers land in the address space.

What is left is narrow: something the V3D driver decides once per process that
changes how expensive it is to *write* a command, with the commands themselves
costing the same to execute. A memory type for the command pool that is
uncached on one path and cached on the other would have exactly this shape, and
that is invisible from this side of the API — settling it wants Mesa
instrumentation. But the search space is now half its size, and the GPU, the
scheduler and the board's thermals are all out of it.

That is a diagnosis of where, not yet of what. Until it is both, establish a
difference on the GLES row, where a three-run cluster is tight to a couple of
hundredths.

**One run is not a measurement: the Vulkan figure lands in one of two speeds.**
Five consecutive runs of one binary, on a cool fanned board minutes after a
power cycle, came back 13.374, 13.820, 13.368, 13.809 and 13.805 ms. Not a
spread — two clusters, 13.37 and 13.81, each internally tight to a few
hundredths, 0.44 ms apart. Which one a run gets appears to be settled when the
process starts and holds for its whole two hundred frames, which is why every
individual run looks impeccable: its own p99 sits within 0.06 ms of its median
and says nothing.

Three percent is the gap, and it is the same size as differences worth
reporting, so a one-run-each comparison can invent one or hide one.

What it is not: heat, and not accumulated session state. It was read as heat
first, because the afternoon's slow numbers followed hours of running at 77 to
85 degrees. Then a fan brought idle to 67 and the number did not move; then a
power cycle brought it to 61 on a fresh boot and it still did not. Two
plausible mechanisms, tested, both wrong. `vcgencmd get_throttled` is still
worth reading after a run and still latches, but a clean reading does not make
two runs comparable.

The rule that follows does not depend on knowing the cause: **alternate the two
builds in one sitting and take at least three runs of each, then check the
clusters do not overlap.** A difference established that way survives whatever
this is. The three percent an uber-shader charges for one more material kind
was measured so: three runs of each build, 13.376/13.419/13.385 against
12.960/13.019/13.006, neither side straying into the other's band, and it
reproduced in a later session. A single run either side would have proved
nothing at that size.

A whole-frame figure seems less sensitive to the drift than the micro-benchmark
paths -- 21.2 ms held across it -- but that is an observation rather than
something to rely on.

## Why the timing baseline lives here and not on the workstation

Asked directly, because "the desktop is too noisy" had been an impression
rather than a number. Three runs of `xtask bench --skip llvmpipe`, release
build, on the development machine — a sixteen-core Ryzen with an integrated
Radeon and a desktop session running — and the spread of the *median* across
those three runs, per row:

| row | min | max | spread |
| --- | --- | --- | --- |
| vulkan tessellated, 1 sample | 0.743 | 0.749 | 0.8% |
| vulkan distance field | 0.804 | 0.818 | 1.7% |
| gles distance field | 0.913 | 0.932 | 2.1% |
| gles tessellated, 1 sample | 0.791 | 0.813 | 2.8% |
| gles tessellated, 4 samples | 1.523 | 1.591 | 4.5% |
| gles full frame | 1.997 | 2.139 | 7.1% |
| vulkan full frame | 2.002 | 2.383 | 19.0% |
| vulkan tessellated, 4 samples | 1.270 | 1.927 | **51.7%** |

Three per cent is the size of a difference worth reporting. A tolerance loose
enough to admit the bottom row would admit anything, and one tight enough to
mean something would fire on half the runs. That is the whole answer: this
machine cannot gate a timing baseline, and the reason is not the tail but the
median — two of eight rows move by more than any regression this project would
be trying to catch.

The tails are worse and are worth seeing once. In these three runs a row with a
median of 2.068 ms reported a ninety-ninth percentile of **374.752 ms**, and
another with a median of 0.748 ms reported 63.432 ms. Those are this process
being descheduled, not a frame taking that long, which is what the bench's own
preamble warns about.

Against that, the Pi 5 on the same day: seven of eight rows within 0.3% of a
recorded baseline across two runs, and the four GLES rows within a tenth of a
per cent. That is five hundred times steadier on the rows that matter, and it
is why the recorded baseline is a board's and why `--check` is run there.

Scoped honestly: this is a statement about *this machine as configured*, with a
compositor competing for the same GPU. A quiet, headless x86 runner might do
better and has not been tried. What is settled is that the workstation someone
is working on is not that machine, and that the cheap version of a perf gate —
record a baseline here, check it in CI — cannot work.

The gate that does work on every commit is a different quantity entirely: see
`crates/impeller-testkit/tests/cost.rs`, which counts what a frame does rather
than timing it.

## A second board, and what it says about reading a green run

A Radxa Zero 3 (RK3566, Mali-G52, Debian 12) is the other target here, and
almost everything above needs adjusting for it. Its name resolves as `.local`
and not `.lan`. It has no `rsync`, so the sysroot comes over `tar` piped
through `ssh` instead. And it needs its *own* sysroot: glibc 2.36 against the
Pi's newer one, so Pi-built binaries will not run there, while binaries built
against this one run on both. Its gcc is 12, so the last link argument ends
`/12` rather than `/14`.

Take the binary paths from `cargo test --no-run --message-format=json`,
filtering for `.profile.test == true` and reading `.executable`. Not from
`ls -t`: a stale binary from an earlier build otherwise gets shipped, and reads
as the change not having worked.

**Vulkan does not reach the Mali GPU there, and a run will not say so.** The
loader lists a `panfrost_icd.json`, and asking for Vulkan with only that ICD
fails inside `enumerate_physical_devices` with `ERROR_INITIALIZATION_FAILED`.
Leave the other ICDs in place and Vulkan succeeds -- on **llvmpipe**, which is
also installed, so a Vulkan suite runs to completion on a software rasterizer
while a GPU sits beside it unused. The backend that does reach the Mali part is
GLES. On it the public API suite is 228 passed, 0 failed.

**Two failures turned up there, and this paragraph got both of them wrong the
first time.** It said Debian 12's llvmpipe was "old enough to be wrong" and that
neither failure was this renderer's. One of those is half right and the other is
backwards, and both were settled by CI rather than by a board.

The clip one is a driver defect, and not an age. An antialiased line writes half
coverage one pixel outside a rectangular clip -- `(55, 87)` where the scissor
begins at 56 -- on llvmpipe at Mesa 15.0.6 and again at Mesa 25.2.8, which is
current. Mesa 26.1.7, RADV and PanVK are clean. The scissor this renderer
records is right either way, and identical whether the paint asks for
antialiasing or not; what differs is that antialiasing opens a multisampled
pass. Half coverage is two of four sample positions, which reads as a per-sample
scissor test half a pixel out. `public_api` probes for it now and skips the clip
half by name where it finds it.

The dithering one was ours. The test compared a dithered eight-bit render
against the same gradient in a half-float target, and a half-float's step near
six tenths is larger than the error being measured -- so it was measuring its
own reference, and its threshold had been fitted to whatever that came to on one
device. Calling it a driver defect was the comfortable reading and the wrong one.

The moral is not about llvmpipe. **A board tells you a device disagrees; it
cannot tell you who is wrong.** Both of these needed a third device and a fourth
driver before the answer was clear, and one of them needed the answer to be
"us".

The other lesson from that board. **`test result: ok` is not a result.** A suite here reported three
passing tests in a quarter of a second having rendered nothing, because the
context it wanted could not be created and the test returned early. Read the
skip lines. `cargo xtask verify` counts them for you on a workstation; running
bare binaries on a board loses that, so grep for `skipping` alongside
`test result`, and read the "drew N of M" line the catalog prints.

**Grepping for them needs `--nocapture`, and forgetting it looks like success.**
A skip is an `eprintln!` inside a test that then passes, and the harness holds
the output of a passing test. Without the flag the skips are not in what you
grep, so the count comes back zero -- which is indistinguishable from a run
that skipped nothing, and is the more reassuring of the two readings. A full
run on the Pi 5 read as zero skips that way and has a hundred and twenty-one.

Count them with `grep -c skipping` and not by adding up a `uniq -c` by eye. The
first number written here was a hundred and fifteen and was wrong twice over:
the categories were mis-added, and the pattern had a colon in it, which four
lines saying "skipping validation assertions" do not.

Most of those say the validation layer is unavailable,
which is a statement about the board rather than about the renderer: the layer
is not packaged there, so the API use those tests make goes unchecked while
their pixels are still compared. The rest are capability gaps that name
themselves -- no device offering advanced blending, a swapchain returning one
image for every acquisition, a C shared library not built beside its test.

## What it found

Recorded because the point of the exercise is not the procedure. Every one of
these was invisible on a workstation, and each was a defect rather than a
tolerance that needed widening.

- **A display controller was chosen without asking which CRTCs a plane can
  drive.** `possible_crtcs` was not consulted, so plane and CRTC were paired by
  order. Pi 5 HDMI went from one mode in five to five in five.
- **The outlier budget had never worked.** `Tolerance::outlier_fraction` is
  documented as the fraction of pixels allowed to exceed the per-channel bound,
  and was counted as the fraction differing at all — so ordinary rounding was
  charged to it, and rounding is what the per-channel bound exists to absorb.
  The board is where it showed: four pixels of sixteen thousand a sample apart
  on a circle's edge, against a budget of sixteen, rejected on the 148 pixels
  that were a single level out.
- **The per-channel bound counted draws rather than stores.** A group is
  rendered into a target of its own and composited out of it, so its fragments
  are quantized twice — two devices whose arithmetic differs in the last bits
  land two levels apart, on interior pixels rather than edges.
- **An aliased edge can fall either side of a pixel center.** Neither
  specification requires two implementations to compute the same matrix-vector
  product to the last bit, and a fused multiply-add is enough to move an edge by
  a hair. Without multisampling there is no partial coverage to soften it, so
  the pixel goes one way here and the other there at full scale.

The tiler also answered an open design question that a desktop had been giving
the wrong answer to: four samples cost 1.17× on V3D against 1.78× on a desktop
GPU, which closes the margin the architecture had been reasoning about.
`docs/architecture.md` carries the measurement.

**The KMS lane runs on a real display controller, not only on VKMS.** All
twenty-five tests in `impeller-present-drm` pass on the Pi 5 -- eleven unit,
nine scanout, and the five that take DRM master and commit a frame. The board
has two display controllers, `vc4` driving HDMI and `drm-rp1-dsi` driving the
panel, and a separate `v3d` render node, which is the split render/display
topology `architecture.md` says VKMS stands in for. It is now checked against
the thing itself rather than only against the stand-in.

They need no display server running to get master, and there is none on this
board. Note that `cargo test --workspace` does *not* build them -- the crate is
reached through the facade's `drm` feature -- so a cross-compiled suite has to
ask for `-p impeller-present-drm` by name or silently leave all twenty-five
behind. Mine did, on the first run: the total was 809 where it should have been
834.

## What no machine here checks

`cargo xtask gate` prints what the suite says it covered, under the totals, and
the lines are worth reading together rather than one at a time. They say the
same thing three ways: **advanced blending is the capability this bench cannot
reach.**

On the workstation two of three devices check fourteen of twenty-nine blend
modes against their reference equations, and only the software Vulkan device
reaches all twenty-nine. On a Raspberry Pi 5 it is fourteen on all three. In CI
it is fourteen on all three as well, its lavapipe being a version whose answer
to the question differs from the one here. So the advanced modes are compared
against their formulas on exactly one device anywhere in this building, and on
none in CI.

The scene counts say it again from the other side. Twenty of the catalog's
plates and six of the corpus's need the extension on *both* sides of a
comparison, and no pair here has it: the catalog compares two hundred and
forty-six of two hundred and sixty-six, the corpus fifty-three of fifty-nine,
and those are ceilings rather than shortfalls. A third real GPU would move them;
nothing else on this bench will.

None of that is asserted against, and none of it is a defect -- a device without
an extension cannot exercise it. It is written down because a suite that passes
says nothing about the difference, and because a regression in the advanced
blend arithmetic would be caught today by one machine.

## Where it stands

All fifty-three test binaries on a Raspberry Pi 5: **784 passed, 0 failed, 0
ignored**, with a hundred and twenty-one announced skips and the catalog
drawing 222 of its 242 scenes across two devices. It was 794 passed and 23 failed the
first time the board was run.

The Pi 4 is a separate case and is not covered by that number. It has no IOMMU,
so Vulkan does not come up on it at all, and its vc4 display controller refuses
to import what this renderer exports for reasons the DRM crate's documentation
states.
