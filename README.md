# valverig-core

The ValveRig engine: a rack of audio modules, the controls you turn while it
runs, and a `process()` that allocates nothing and takes no lock.

It wraps `valverig-nam`'s captures and `valverig-dsp`'s stages as modules,
runs them in series and in parallel lanes, and builds the whole rack from a
`.rig` preset. It opens no files and no devices: you load the data and hand
it over.

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

## Tests

```bash
cargo test
cargo test --release
```

`assets/` holds four `.nam` captures and one `.rig` preset, so the tests run
against a real capture and a real document without needing any other
repository beside this one. Impulse responses are generated in the tests
themselves. `a2.nam` and `wavenet_a1_standard.nam` are test fixtures from
[NeuralAmpModelerCore](https://github.com/sdatkinson/NeuralAmpModelerCore),
MIT licensed, © 2023 Steven Atkinson; `lstm.nam` and `wavenet.nam` are from
the same source, modified.

The benchmark is a test that measures rather than checks, so it is ignored
by default:

```bash
cargo test --release --test benchmark -- --ignored --nocapture
```

## How to use

**It allocates nothing, takes no lock, and does no I/O**, on the calling
thread and on the lane workers alike. `tests/realtime.rs` holds it there
with a counting allocator while a host turns every knob and switch between
blocks.

**Any number of frames.** More than the rack was prepared for is split into
blocks of at most that many and run in turn, because CoreAudio and others do
over-deliver, and an audio callback is no place to find that out. Zero
frames does nothing.

**The output does not depend on how frames are grouped.** The same signal
through the same rack gives the same samples whether it arrives in blocks of
1, of 128, or in a ragged mixture. This is tested bit for bit.

**Several settings are exactly transparent**, so a rack can be tested
against its parts run by hand: a gain at 0 dB, a flat tone control, a reverb
or a pedal at a mix of zero, and a cabinet with both cuts parked at the ends
of their ranges all pass their input through bit for bit.

**Denormals are flushed for the length of the call.** The floating-point
control register belongs to the host's thread, so it is set on the way in
and put back exactly as it was found on the way out, including out of a
panic.

**A non-finite block never leaves.** One `NaN` reaching a feedback path
would stay there and poison every block after it: a reverb's recirculation,
a delay line, a capture's recurrent state. `process` checks what it
produced; on anything not finite it silences that block, clears what can be
cleared inside a callback, and sets `Rack::faulted()`. Everything but a
capture recovers on the spot. A capture's recovery means running its prewarm
length of silence through it, which is not callback work, so that one is
left to `Rack::reset` from the thread that prepared the rack.

**It panics on two caller mistakes**: an `output` that does not hold exactly
`Rack::channels_out()` slices, and any output slice of a different length
from the input. Both would otherwise surface as silence or as a slice index.

`prepare` is the opposite of all this: it allocates, and it belongs on the
thread that builds the rack. So does `reset`, which allocates nothing but is
not bounded in time, because a capture settles itself on silence there.

### What is refused

**A sample rate a module cannot run at.** Nothing here resamples. A capture
runs at the rate it was trained at; an impulse response played at the wrong
rate is a different cabinet. Both refuse `prepare` at another rate and name
themselves in the error, and a host that runs at a different rate converts
at the rack's edges.

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
same as any other set.

### Controls, latency and parallel lanes

`Controls` is cheap to clone and `Send + Sync`; it stays with the host when
the rack moves to the audio thread. Every parameter has a stable string id,
never an index. A set is one clamped atomic store, and the module reads it
on its next block and glides where a step would click.

`Rack::latency()` sums the stages, a lanes stage counting as its longest
lane, with every module counted whether or not it is bypassed so the figure
does not move under a host. The shorter lanes of a stage are padded up to
the longest at `prepare`, so this is what every path through the rack takes.

`Rack::set_parallel_lanes(true, hook)` before `prepare` runs every lane but
the first on a thread of its own. The result is bit for bit the sequential
one, in the same mix order, in ragged blocks, with lanes bypassed under it -
`tests/parallel.rs` holds that. Workers spin briefly for the next block and
then park; `hook` runs once at the start of each, for the host to raise it
to the audio thread's priority, which matters on a busy machine where an
ordinary-priority worker can be held off a core while the audio thread waits
for it.

## Licence

MIT, in [LICENSE](LICENSE).
