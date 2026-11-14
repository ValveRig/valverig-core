# valverig-core

The ValveRig engine: a rack of audio modules, the controls you turn while it
runs, the DSP the modules are made of, and the `.rig` presets that describe
a rack. Its `process()` allocates nothing and takes no lock.

It runs `valverig-nam`'s captures and its own gate, compressor, tone
control, drive, chorus, phaser, delay, reverb and cabinet as modules, in
series and in parallel lanes, and builds the whole rack from a `.rig`
document. It opens no files: you load the data and hand it over. It opens no
device either, unless you ask for the `io` feature, which adds the one
module that does: an audio device opened, and a rack run in its callback.

```rust
use valverig_core::capture::Capture;
use valverig_core::eq::Tone;
use valverig_core::gain::Gain;
use valverig_core::rack::Rack;
use valverig_nam::loader::Model;

let (mut rack, controls) = Rack::builder()
    .push("input", Gain::new())?
    .push("amp", Capture::new(Model::from_file("plexi.nam")?))?
    .push("eq", Tone::new())?
    .push("output", Gain::new())?
    .build();

rack.prepare(48_000.0, 128)?;             // allocates; before the audio thread starts
controls.set("output/level_db", -6.0)?;   // from any thread, any time

let dry = [0.0f32; 128];
let mut wet = [0.0f32; 128];
rack.process(&dry, &mut [&mut wet]);      // on the audio thread
```

`Rack::builder().split(lanes)` appends a parallel stage instead: the signal
goes into every lane, each lane runs its own modules, and the mix pans each
lane's result into a stereo pair. `preset::build` does all of it from a
`.rig` document, and `preset::snapshot` writes a running rack back into one.
Every stage also runs without a rack: `drive::Mono`, `reverb::Fdn`,
`dsp::convolution::Convolver` and the rest take a block and return one.
`dsp::pitch::Detector` is the one piece that sits beside the signal path
and not in it: it reads the pitch of a string from a copy of the input, to
the cent, which is what a tuner needs.

## Tests

```bash
cargo test --all-features
cargo test --release --all-features
cargo test --release --all-features -- --ignored --nocapture   # also plays a tone on the default device
```

Both builds, because SIMD and float behaviour differ between the two and the
bit-exactness tests have to hold in each. Every feature, because the device
side (`io`) is tested too: all but the last line needs no sound hardware.

`assets/` holds four `.nam` captures and one `.rig` preset, so the tests run
against a real capture and a real document without needing any other
repository beside this one. Impulse responses and every other signal are
generated in the tests themselves. `a2.nam` and `wavenet_a1_standard.nam`
are test fixtures from
[NeuralAmpModelerCore](https://github.com/sdatkinson/NeuralAmpModelerCore),
MIT licensed, © 2023 Steven Atkinson; `lstm.nam` and `wavenet.nam` are from
the same source, modified. `plexi-tweed-wide.rig` was written by hand for
this crate: two captures in parallel into one reverb, exercising every shape
the format has. Its capture hashes are the real content addresses of
`a2.nam` and `wavenet_a1_standard.nam`; the cabinet's is a placeholder.

Two tests measure rather than check, the convolver's partition sweep and a
rack's cost per block, so they are ignored by default:

```bash
cargo test --release --test benchmark -- --ignored --nocapture
```

## How to use

**Nothing on the audio path allocates, takes a lock, or does I/O**, on the
calling thread and on the lane workers alike. `tests/realtime.rs` holds every
stage and every rack to that with a counting allocator while every knob and
switch turns between blocks. `prepare` is the opposite: it allocates, and it
belongs on the thread that builds the rack. So does `reset`, which allocates
nothing but is not bounded in time, because a capture settles itself on
silence there.

**Any number of frames.** More than the rack was prepared for is split into
blocks of at most that many and run in turn, because CoreAudio and others do
over-deliver, and an audio callback is no place to find that out. Zero
frames does nothing. A stage on its own asks for no maximum at all.

**The output does not depend on how frames are grouped.** The same signal
through the same stage or rack gives the same samples whether it arrives in
blocks of 1, of 128, or in a ragged mixture. Every stage's tests and the
rack's hold that bit for bit.

**Denormals are flushed for the length of the call.** The floating-point
control register belongs to the host's thread, so `Rack::process` sets it on
the way in and puts it back exactly as it was found on the way out,
including out of a panic. The lane workers, whose threads the rack owns, set
it once.

**A non-finite block never leaves.** One `NaN` reaching a feedback path
would stay there and poison every block after it: a reverb's recirculation,
a delay line, a capture's recurrent state. `process` checks what it
produced; on anything not finite it silences that block, clears what can be
cleared inside a callback, and sets `Rack::faulted()`. Everything but a
capture recovers on the spot. A capture's recovery means running its prewarm
length of silence through it, which is not callback work, so that one is
left to `Rack::reset` from the thread that prepared the rack. A stage run on
its own has no such guard: whoever owns its input owns that check.

**It panics on two caller mistakes**: an `output` that does not hold exactly
`Rack::channels_out()` slices, and any output slice of a different length
from the input. Both would otherwise surface as silence or as a slice index.

### What is exact

Several settings are transparent bit for bit, not merely inaudibly: a gain
at 0 dB, an open gate, a compressor at rest with no makeup, a flat tone
control, a mix of exactly zero on the reverb, chorus, phaser and delay, and a
cabinet with both cuts parked at the ends of their ranges. That is what lets
a rack be tested against its parts run by hand.

Getting there needs a smoothed parameter to land *on* its target rather than
near it, because a one-pole in `f32` stalls short of it once each step
rounds to nothing, so every glide snaps when it comes within a documented
distance. See `src/dsp/one_pole.rs`.

### What is refused

**A sample rate outside 1 kHz to 768 kHz.** `Rack::prepare` returns an error
for it, a `.rig` document stating one is refused when it is read, and a
stage built on its own panics, NaN and infinity included. The reverb, the
chorus and the delay size a buffer from the rate, so a rate that is quietly
nonsense would make a buffer that is quietly wrong.

**A sample rate a module cannot run at.** Nothing here resamples. A capture
runs at the rate it was trained at, and an impulse response played at the
wrong rate is a different cabinet. Both refuse `prepare` at another rate and
name themselves in the error; a host that runs at a different rate converts
at the rack's edges, and `dsp::resample` is there for that.

**A capture after a mix.** The rack is mono in, mono until the first lanes
stage and stereo after it, and every module is prepared for the width it
will see. A capture is mono and cannot be widened, so one placed after a
lanes stage refuses rather than silently costing twice.

**Two modules or lanes with the same name.** Modules and lanes share one
namespace, because a name is the first half of every control address:
`output/level_db`, `A/pan`, `A.eq/bass_db`.

**A preset the engine cannot build**: a module type it does not have, a
parameter a module does not have, an asset of the wrong kind. Values out of
range are not refused; they are clamped to the parameter's descriptor, the
same as any other set. Mixes, depths, feedbacks and ratios set on a stage
directly are clamped the same way.

### Tails end

A reverb tail, a filter's ringing and a peak detector all decay towards zero
without arriving, so each stage cuts its own once what is left cannot affect
the output, at 1e-20 for filter state and 1e-15 for what goes back into a
delay line. A two-second reverb reaches exact silence after 8.3 s rather
than about 23 s, and a tone control returned to flat resumes its bit-exact
copy-through a fifth of a second sooner. Those floors are not denormal
handling, which is the flush-to-zero above: the subnormal range begins
eighteen decades below them, and they work with no mode set at all, on any
thread.

### Controls, latency and parallel lanes

`Controls` is cheap to clone and `Send + Sync`; it stays with the host when
the rack moves to the audio thread. Every parameter has a stable string id,
never an index. A set is one clamped atomic store, and the module reads it
on its next block and glides where a step would click.

`Rack::latency()` sums the stages, a lanes stage counting as its longest
lane, with every module counted whether or not it is bypassed so the figure
does not move under a host. The shorter lanes of a stage are padded up to
the longest at `prepare`, so this is what every path through the rack takes.
Every stage here reports zero, the convolver included: its first partition
runs in the time domain, so a cabinet adds no delay at all. The module doc
of `dsp::convolution` has how, and what it costs.

`Rack::set_parallel_lanes(true, hook)` before `prepare` runs every lane but
the first on a thread of its own. The result is bit for bit the sequential
one, in the same mix order, in ragged blocks, with lanes bypassed under it:
`tests/parallel.rs` holds that. Workers spin briefly for the next block and
then park; `hook` runs once at the start of each, for the host to raise it
to the audio thread's priority, which matters on a busy machine where an
ordinary-priority worker can be held off a core while the audio thread waits
for it.

### The `.rig` document

```toml
format_version = 1
name = "Plexi + Tweed · Wide"
author = "ValveRig"                # optional, as is `notes`

[assets.plexi]                     # what to load, by content, not by path
kind = "nam"                       # "nam" or "ir"
hash = "blake3:586a3e4f…"          # 64 hex digits: the BLAKE3 of the bytes
file = "brit-plexi-50-v2.nam"      # a file name, to help recognise it

[[stage]]                          # stages in series; this one is a module
type = "std.gate"
name = "gate"
bypass = true                      # out of the path; default false
[stage.params]
threshold_db = -50.0
[stage.ui]                         # presentation hints the engine ignores
label = "Quiet Room"

[[stage]]                          # this one is lanes in parallel
[[stage.lane]]
name = "A"
pan = -1.0                         # -1 hard left, 0 centre, 1 hard right
level_db = 0.0
[[stage.lane.module]]
type = "std.capture"
name = "A.capture"
asset = "plexi"
```

A parameter the preset does not set stays at the module's default.

**An asset is found by content.** The `hash` is the identity. The `file` is
a plain file name and is refused if it is anything else: no `/`, no `\`, no
`.` or `..`, no absolute path. A resolver chooses which directories to
search and can join the hint to one of them without a document being able
to walk out of it; the hash is what confirms it found the right bytes.

**A round trip cannot lose anything.** A key this reader does not know is
refused rather than ignored, because a key ignored on the way in is a key
deleted on the way out. Anything written can be read back, every `f64` bit
for bit. Version 1 is the only version read or written.

**Nothing panics on a bad document.** Every malformed input returns an
`Error` naming the part at fault, and every count a document states is
capped where it is still just text.

### A device, with the `io` feature

```toml
valverig-core = { version = "0.0.2", features = ["io"] }
```

```rust
use valverig_core::io::{Config, InputChannel, run};

let stream = run(
    Config {
        sample_rate: 48_000,
        block: 128,
        live_input: true,
        output_channels: 2,
        output_device: None,   // the host's default
        input_device: None,
        input_channel: InputChannel::First,
    },
    // On the audio thread: no allocating, no locking, no logging, no waiting.
    move |input, outputs| rack.process(input, outputs),
)?;

println!("playing on {}", stream.device);
drop(stream);   // and the device is released
```

One closure, mono in and one or two channels out, with no queue in between,
because a queue is latency and a guitarist hears it. The device decides how
many frames it hands over, so whatever arrives is cut into pieces of at most
`Config::block`: a rack never sees more than it was prepared for. It is off
by default because a plugin or a worklet is handed its audio and has no use
for a host's device layer; with it comes `cpal`, and on Linux ALSA's
headers.

**Nothing here resamples.** `Config::sample_rate` is the rate the rack
itself needs. A device that does not offer it is an error rather than
something to convert around, so a rack at another rate is converted at its
own edges, with `dsp::resample`.

**Live input is exactly one block late**, and there is no way to ask for
less: input and output are two separate streams, and the ring between them
is primed with one block of silence so the output never waits on the input
callback that runs just after it.

**Input and output are two clocks.** Use one device for both, or accept that
two that disagree by even a few parts per million will slowly fill the ring
or empty it, and the frames it cannot supply arrive as silence. Nothing here
regulates that drift.

**A non-finite sample never reaches the driver**, and denormals are flushed
to zero on the audio thread. A rack that returns a NaN has a poisoned
feedback structure and will not recover on its own: silence is the only
honest thing to send a speaker.

**Nothing is reported once a stream is running.** A device unplugged under
the stream raises an error the host may deliver on the audio thread, where
nothing can log it or allocate a message for it. Watch for the callback
going quiet, then drop the `Stream` and open another: there is no reset and
no route-change notification.

**Configuration is checked before the host is touched**, so a block size of
zero or a nonsense channel count out of a settings file is an error and not
a panic on the audio thread.

## Licence

MIT, in [LICENSE](LICENSE).

Three published designs are implemented here, and credited where they are
used: the second-order sections come from Robert Bristow-Johnson's Audio EQ
Cookbook, the reverb's input diffusion from Jon Dattorro's *Effect Design,
Part 1: Reverberator and Other Filters* (1997), and the compressor from
Giannoulis, Massberg and Reiss, *Digital Dynamic Range Compressor Design: A
Tutorial and Analysis* (2012). The fixtures in `assets/` are described under
Tests.
