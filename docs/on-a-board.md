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

`IMPELLER_DRM_CARD` matters on a board with more than one display controller. A
Pi 5 has two, and a test that opens the first `/dev/dri/cardN` gets `rp1-dsi`
rather than `vc4`. A suite that passes on the controller that works says nothing
about the one beside it; `impeller-present-drm`'s crate documentation has the
table of what each board actually does.

## Benchmarking there, which has two rules of its own

The suite above runs from a debug build and should. `cargo xtask bench` must
not, and refuses to: cross-build it with `--release` and copy that binary.

```sh
cargo build -p xtask --release --target aarch64-unknown-linux-gnu
scp target/aarch64-unknown-linux-gnu/release/xtask "$PI:/tmp/xtask"
ssh "$PI" 'chmod +x /tmp/xtask && /tmp/xtask bench --skip llvmpipe'
```

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

**Read `vcgencmd get_throttled` after the run, not only before.** A Pi 5 that
has been benching for an hour sits near its soft temperature limit, and the
same binary then measures a few percent slower than it did on a cool board --
with the run's own spread still under a tenth of a millisecond, so nothing
inside the numbers says anything is wrong. That is how a thermal difference
gets read as a regression: two runs, each internally tight, disagreeing by four
percent. `throttled=0x80000` is bit nineteen, "soft temperature limit has
occurred", and it latches, so it answers the question after the fact.

The rule that follows: compare two builds in one sitting, alternating them, or
let the board come back to idle temperature between them. Comparing today's run
against a number from an hour ago is comparing two thermal states as much as
two builds. A whole-frame figure seems less sensitive to this than the
micro-benchmark paths -- 21.2 ms was unchanged across a four percent move in
them -- but that is an observation rather than something to rely on.

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
thirty-nine of two hundred and fifty-nine, the corpus fifty-one of fifty-seven,
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
