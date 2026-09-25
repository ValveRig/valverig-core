//! Modules in a rack: stages in series, lanes in parallel, and the controls
//! on all of it.
//!
//! A [`Rack`] is an ordered list of stages. A stage is one module, or a set
//! of [`Lane`]s: the signal is split into every lane, each lane runs its own
//! modules in series, and the mix pans each lane's result into a stereo pair
//! by the lane's level and pan. The rack is mono in. It is mono up to the
//! first lanes stage and stereo after it, and every module is prepared for
//! the width it will see, so an amp in a lane costs one amp, and the reverb
//! after the mix keeps the pair.
//!
//! A rack is built once, prepared, and moved to the audio thread. The
//! [`Controls`] it is built with stay behind: a cheap, cloneable handle that
//! sets any parameter or bypass by a stable address from any thread, through
//! the atomics the modules read. Modules and lanes share one namespace of
//! names, and a name is the first half of every address: `A.amp/level_db`,
//! `A/pan`, `reverb/mix`.
//!
//! A two-amp rig is one lanes stage between a mono front end and a stereo
//! back end, but nothing here knows that: a rack is any sequence of stages,
//! lanes may follow lanes, and a lane may hold anything a rack may. Routing
//! that is not a series of stages, such as a graph or a feedback path
//! around a stage, is the one shape a rack cannot express.
//!
//! Lanes can run on threads of their own with
//! [`Rack::set_parallel_lanes`]: the first on the thread that calls
//! `process`, the rest on workers that wait for each block, so a block
//! takes as long as the longest lane. The result is the sequential one bit
//! for bit, and the audio path still does not allocate.
//!
//! Two things cross the stages rather than pass through them. A
//! [`Tap`](crate::tap::Tap) reaches the output: once every stage has run,
//! the rack calls [`Node::tap`] on every module that ran, in rack order,
//! with the output buffers, and a tap adds its copy of the block there. A
//! [`BusSend`](crate::bus::BusSend) puts its block on one of [`BUSES`]
//! buses, taken by the rack right after the send ran or after its lanes
//! stage was joined, and a [`BusReturn`](crate::bus::BusReturn) later in
//! rack order is handed that block before it runs.
//!
//! Lanes of unequal latency are aligned: at `prepare`, every lane of a stage
//! is padded with a delay of the longest lane's latency less its own, so
//! the mix adds what left the split together.

use crate::dsp::{decibels, one_pole};
use crate::node::{Node, Param, ParamSpec, Unit};
use crate::{Error, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use crate::dsp::denormals::Restoring;
use crate::worker::{DONE, GO, Hook, IDLE, QUIT, Shared, worker};

/// A lane's level into the mix.
pub const LANE_LEVEL: ParamSpec = ParamSpec {
    id: "level_db",
    name: "Level",
    unit: Unit::Decibels,
    min: -60.0,
    max: 12.0,
    default: 0.0,
};

/// A lane's place in the pair: `-1` hard left, `0` centre, `1` hard right.
/// Constant power for a mono lane, so the centre is 3 dB down on each side
/// and a hard pan is exact; a balance for a stereo lane.
pub const LANE_PAN: ParamSpec = ParamSpec {
    id: "pan",
    name: "Pan",
    unit: Unit::Ratio,
    min: -1.0,
    max: 1.0,
    default: 0.0,
};

/// Runs at the start of every lane worker thread, for the host to raise
/// the thread's priority to the audio thread's. See
/// [`Rack::set_parallel_lanes`].
pub type ThreadHook = Arc<dyn Fn() + Send + Sync>;

/// The type id lanes report in [`Controls::modules`].
pub const LANE_TYPE_ID: &str = "std.lane";

/// How many buses a rack has for its sends and returns.
pub const BUSES: usize = 4;

/// One bus: the last block sent on it, and how wide that was.
#[derive(Debug)]
struct Bus {
    data: [Vec<f32>; 2],
    width: usize,
    frames: usize,
}

impl Bus {
    fn new(max_block: usize) -> Self {
        Self {
            data: [vec![0.0; max_block], vec![0.0; max_block]],
            width: 1,
            frames: 0,
        }
    }

    fn take(&mut self, block: [&[f32]; 2]) {
        let n = block[0].len().min(self.data[0].len());
        self.data[0][..n].copy_from_slice(&block[0][..n]);
        if block[1].len() >= n && !block[1].is_empty() {
            self.data[1][..n].copy_from_slice(&block[1][..n]);
            self.width = 2;
        } else {
            self.width = 1;
        }
        self.frames = n;
    }

    fn hand_to(&self, node: &mut dyn Node) {
        let n = self.frames;
        node.receive([
            &self.data[0][..n],
            &self.data[1][..if self.width == 2 { n } else { 0 }],
        ]);
    }
}

/// The mix follows a level or pan change with this one-pole time.
const MIX_SMOOTH_MS: f32 = 5.0;

#[derive(Debug)]
struct Module {
    name: String,
    node: Box<dyn Node>,
    bypass: Arc<AtomicBool>,
    /// Whether the module ran in the block just processed, so that only
    /// those are tapped: a bypass flipped under the rack must not tap a
    /// stale block.
    ran: bool,
}

impl Module {
    fn new(name: &str, node: Box<dyn Node>) -> Self {
        Self {
            name: name.into(),
            node,
            bypass: Arc::new(AtomicBool::new(false)),
            ran: false,
        }
    }
}

/// The gains into the left and right of a pair for a signal `width` wide
/// at `pan`, times `level`: a constant-power pan of a mono signal, so hard
/// left and hard right are exact and the centre is 3 dB down on each side,
/// and a balance of a stereo one.
pub(crate) fn pan_gains(width: usize, pan: f32, level: f32) -> [f32; 2] {
    let (l, r) = if width == 1 {
        if pan <= -1.0 {
            (1.0, 0.0)
        } else if pan >= 1.0 {
            (0.0, 1.0)
        } else {
            let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
            (angle.cos(), angle.sin())
        }
    } else if pan <= 0.0 {
        (1.0, 1.0 + pan)
    } else {
        (1.0 - pan, 1.0)
    };
    [level * l, level * r]
}

/// Run one module from `src` to `dst`, `width` channels of `n` frames each.
fn run_module(
    node: &mut dyn Node,
    src: &[Vec<f32>; 2],
    dst: &mut [Vec<f32>; 2],
    width: usize,
    n: usize,
) {
    let input: [&[f32]; 2] = [&src[0][..n], &src[1][..n]];
    let [d0, d1] = dst;
    let mut output: [&mut [f32]; 2] = [&mut d0[..n], &mut d1[..n]];
    node.process(&input[..width], &mut output[..width]);
}

fn label(e: Error, name: &str) -> Error {
    match e {
        Error::SampleRate { node, needs, got } => Error::SampleRate {
            node: format!("{name} ({node})"),
            needs,
            got,
        },
        Error::Channels { node, max, got } => Error::Channels {
            node: format!("{name} ({node})"),
            max,
            got,
        },
        other => other,
    }
}

/// One lane of a lanes stage: modules in series, then a level and a pan into
/// the mix. Build it with [`Lane::new`] and [`Lane::push`], set its place
/// with [`Lane::pan`] and [`Lane::level_db`], and hand it to
/// [`RackBuilder::split`].
#[derive(Debug)]
pub struct Lane {
    name: String,
    modules: Vec<Module>,
    bypass: Arc<AtomicBool>,
    level: Arc<Param>,
    pan: Arc<Param>,
    /// The smoothed gains into the left and right of the mix.
    gain: [f32; 2],
    coeff: f32,
    width: usize,
    a: [Vec<f32>; 2],
    b: [Vec<f32>; 2],
    /// Whether the lane ran in the block just processed.
    ran: bool,
    /// The delay that lines this lane up with the longest in its stage, in
    /// frames, and the line that holds it.
    pad: usize,
    pad_line: [Vec<f32>; 2],
    pad_pos: usize,
}

impl Lane {
    /// An empty lane called `name`, centred at 0 dB.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            modules: Vec::new(),
            bypass: Arc::new(AtomicBool::new(false)),
            level: Param::new(LANE_LEVEL),
            pan: Param::new(LANE_PAN),
            gain: [0.0; 2],
            coeff: 1.0,
            width: 1,
            a: [Vec::new(), Vec::new()],
            b: [Vec::new(), Vec::new()],
            ran: false,
            pad: 0,
            pad_line: [Vec::new(), Vec::new()],
            pad_pos: 0,
        }
    }

    /// Append `node` as the module called `name`.
    pub fn push(mut self, name: &str, node: impl Node + 'static) -> Result<Self> {
        if self.modules.iter().any(|m| m.name == name) || self.name == name {
            return Err(Error::DuplicateName(name.into()));
        }
        self.modules.push(Module::new(name, Box::new(node)));
        Ok(self)
    }

    /// Start the lane at this pan.
    pub fn pan(self, pan: f32) -> Self {
        self.pan.set(pan);
        self
    }

    /// Start the lane at this level.
    pub fn level_db(self, db: f32) -> Self {
        self.level.set(db);
        self
    }

    /// The gains the lane is heading for: a constant-power pan of a mono
    /// lane, a balance of a stereo one, times the level.
    fn targets(&self) -> [f32; 2] {
        pan_gains(
            self.width,
            self.pan.get(),
            decibels::to_linear(self.level.get()),
        )
    }

    fn prepare(&mut self, sample_rate: f64, max_block: usize, width: usize) -> Result<()> {
        for m in &mut self.modules {
            m.node
                .prepare(sample_rate, max_block, width)
                .map_err(|e| label(e, &m.name))?;
        }
        self.width = width;
        self.coeff = one_pole::from_time(MIX_SMOOTH_MS, sample_rate as f32);
        self.gain = self.targets();
        self.a = [vec![0.0; max_block], vec![0.0; max_block]];
        self.b = [vec![0.0; max_block], vec![0.0; max_block]];
        Ok(())
    }

    fn reset(&mut self) {
        for m in &mut self.modules {
            m.node.reset();
        }
        self.clear_mix_and_pad();
    }

    /// [`Lane::reset`] without the part that takes real time.
    fn silence(&mut self) {
        for m in &mut self.modules {
            m.node.silence();
        }
        self.clear_mix_and_pad();
    }

    fn clear_mix_and_pad(&mut self) {
        self.gain = self.targets();
        for line in &mut self.pad_line {
            line.fill(0.0);
        }
        self.pad_pos = 0;
    }

    /// The lane's own delay: its modules' latencies summed.
    fn latency(&self) -> usize {
        self.modules.iter().map(|m| m.node.latency()).sum()
    }

    /// Delay the lane's result by `pad` frames, so that it lines up with a
    /// longer lane beside it. Allocates.
    fn set_pad(&mut self, pad: usize) {
        self.pad = pad;
        self.pad_line = [vec![0.0; pad], vec![0.0; pad]];
        self.pad_pos = 0;
    }

    /// Run the modules on `src`; the result is in `a` when this returns
    /// true, else in `b`.
    pub(crate) fn run(&mut self, src: [&[f32]; 2], n: usize) -> bool {
        for (dst, s) in self.a.iter_mut().zip(src).take(self.width) {
            dst[..n].copy_from_slice(&s[..n]);
        }
        let mut in_a = true;
        self.ran = true;
        for m in &mut self.modules {
            m.ran = !m.bypass.load(Ordering::Relaxed);
            if !m.ran {
                continue;
            }
            if in_a {
                run_module(m.node.as_mut(), &self.a, &mut self.b, self.width, n);
            } else {
                run_module(m.node.as_mut(), &self.b, &mut self.a, self.width, n);
            }
            in_a = !in_a;
        }
        if self.pad > 0 {
            // Through the line: what comes out is what went in `pad` frames ago.
            let dst = if in_a { &mut self.a } else { &mut self.b };
            for (buf, line) in dst.iter_mut().zip(&mut self.pad_line).take(self.width) {
                let mut pos = self.pad_pos;
                for v in buf[..n].iter_mut() {
                    std::mem::swap(&mut line[pos], v);
                    pos += 1;
                    if pos == self.pad {
                        pos = 0;
                    }
                }
            }
            self.pad_pos = (self.pad_pos + n) % self.pad;
        }
        in_a
    }

    /// Add the lane's result, panned and levelled, into `dst`.
    fn mix_into(&mut self, in_a: bool, dst: &mut [Vec<f32>; 2], n: usize) {
        let src = if in_a { &self.a } else { &self.b };
        let target = self.targets();
        let snap = target.map(one_pole::gain_snap);
        let coeff = self.coeff;
        let [mut gl, mut gr] = self.gain;
        let [dl, dr] = dst;
        let right_source = if self.width == 1 { 0 } else { 1 };
        for i in 0..n {
            gl = one_pole::step(gl, target[0], coeff, snap[0]);
            gr = one_pole::step(gr, target[1], coeff, snap[1]);
            dl[i] += src[0][i] * gl;
            dr[i] += src[right_source][i] * gr;
        }
        self.gain = [gl, gr];
    }
}

#[derive(Debug)]
enum Stage {
    Module(Module),
    Lanes(Vec<Arc<Shared>>),
}

/// What a rack was last prepared for.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Prepared {
    sample_rate: f64,
    max_block: usize,
}

/// Stages in series, run in one `process`.
#[derive(Debug)]
pub struct Rack {
    stages: Vec<Stage>,
    a: [Vec<f32>; 2],
    b: [Vec<f32>; 2],
    buses: Vec<Bus>,
    prepared: Option<Prepared>,
    channels_out: usize,
    parallel: bool,
    hook: Hook,
    workers: Vec<JoinHandle<()>>,
    /// Set when a block came out non-finite; cleared by [`Rack::reset`].
    faulted: Arc<AtomicBool>,
}

/// Somewhere a module can be appended: a rack under construction, or one
/// of its lanes. The preset registry appends through this, so a module
/// built from a file is stored exactly as one pushed by hand and costs the
/// same single indirection to run.
pub(crate) trait Append: Sized {
    fn append(self, name: &str, node: impl Node + 'static) -> Result<Self>;
}

impl Append for RackBuilder {
    fn append(self, name: &str, node: impl Node + 'static) -> Result<Self> {
        self.push(name, node)
    }
}

impl Append for Lane {
    fn append(self, name: &str, node: impl Node + 'static) -> Result<Self> {
        self.push(name, node)
    }
}

/// Collects stages; [`RackBuilder::build`] hands back the rack and its
/// controls.
#[derive(Debug, Default)]
pub struct RackBuilder {
    stages: Vec<Stage>,
    names: Vec<String>,
}

impl RackBuilder {
    fn claim(&mut self, name: &str) -> Result<()> {
        if self.names.iter().any(|n| n == name) {
            return Err(Error::DuplicateName(name.into()));
        }
        self.names.push(name.into());
        Ok(())
    }

    /// Append `node` as the module called `name`.
    pub fn push(mut self, name: &str, node: impl Node + 'static) -> Result<Self> {
        self.claim(name)?;
        self.stages
            .push(Stage::Module(Module::new(name, Box::new(node))));
        Ok(self)
    }

    /// Append a lanes stage: the signal into every lane, the lanes' results
    /// panned into a stereo pair.
    pub fn split(mut self, lanes: Vec<Lane>) -> Result<Self> {
        if lanes.is_empty() {
            return Err(Error::NoLanes);
        }
        for lane in &lanes {
            self.claim(&lane.name)?;
            for m in &lane.modules {
                self.claim(&m.name)?;
            }
        }
        self.stages
            .push(Stage::Lanes(lanes.into_iter().map(Shared::new).collect()));
        Ok(self)
    }

    /// The rack, unprepared, and the controls on it.
    pub fn build(self) -> (Rack, Controls) {
        let mut entries = Vec::new();
        for stage in &self.stages {
            match stage {
                Stage::Module(m) => entries.push(Entry::module(m)),
                Stage::Lanes(lanes) => {
                    for cell in lanes {
                        // SAFETY: `build` consumes the builder, so nothing
                        // else holds these cells and no worker exists yet.
                        let lane = unsafe { cell.lane_held() };
                        // So that `Rack::latency` answers before `prepare`
                        // as well as after it.
                        cell.latency.store(lane.latency(), Ordering::Relaxed);
                        entries.push(Entry {
                            name: lane.name.clone(),
                            type_id: LANE_TYPE_ID,
                            bypass: lane.bypass.clone(),
                            params: vec![lane.level.clone(), lane.pan.clone()],
                        });
                        entries.extend(lane.modules.iter().map(Entry::module));
                    }
                }
            }
        }
        let channels_out = if self.stages.iter().any(|s| matches!(s, Stage::Lanes(_))) {
            2
        } else {
            1
        };
        let rack = Rack {
            stages: self.stages,
            a: [Vec::new(), Vec::new()],
            b: [Vec::new(), Vec::new()],
            buses: Vec::new(),
            prepared: None,
            channels_out,
            parallel: false,
            hook: Hook::default(),
            workers: Vec::new(),
            faulted: Arc::new(AtomicBool::new(false)),
        };
        (
            rack,
            Controls {
                inner: Arc::new(entries),
            },
        )
    }
}

impl Rack {
    /// Start an empty rack.
    pub fn builder() -> RackBuilder {
        RackBuilder::default()
    }

    /// Prepare every module for blocks of up to `max_block` frames at
    /// `sample_rate`, each for the width of the signal where it sits.
    /// Allocates; call it before the audio thread starts, and again on any
    /// change. A module that cannot run there says so, and a rate outside
    /// [`MIN_SAMPLE_RATE`](crate::MIN_SAMPLE_RATE) to
    /// [`MAX_SAMPLE_RATE`](crate::MAX_SAMPLE_RATE) is refused before any
    /// module sees it.
    pub fn prepare(&mut self, sample_rate: f64, max_block: usize) -> Result<()> {
        if !(crate::MIN_SAMPLE_RATE..=crate::MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(Error::UnsupportedSampleRate(sample_rate));
        }
        self.stop_workers();
        let mut width = 1;
        for stage in &mut self.stages {
            match stage {
                Stage::Module(m) => {
                    m.node
                        .prepare(sample_rate, max_block, width)
                        .map_err(|e| label(e, &m.name))?;
                }
                Stage::Lanes(lanes) => {
                    for cell in lanes.iter() {
                        // SAFETY: `&mut self`, and `stop_workers` above left
                        // no worker running, so this thread owns the lane.
                        let lane = unsafe { cell.lane_held() };
                        lane.prepare(sample_rate, max_block, width)?;
                        cell.latency.store(lane.latency(), Ordering::Relaxed);
                    }
                    // Line the lanes up: each waits for the longest.
                    let longest = lanes
                        .iter()
                        .map(|cell| cell.latency.load(Ordering::Relaxed))
                        .max()
                        .unwrap_or(0);
                    for cell in lanes.iter() {
                        // SAFETY: as above.
                        let lane = unsafe { cell.lane_held() };
                        lane.set_pad(longest - lane.latency());
                    }
                    width = 2;
                }
            }
        }
        self.a = [vec![0.0; max_block], vec![0.0; max_block]];
        self.b = [vec![0.0; max_block], vec![0.0; max_block]];
        self.buses = (0..BUSES).map(|_| Bus::new(max_block)).collect();
        self.prepared = Some(Prepared {
            sample_rate,
            max_block,
        });
        if self.parallel {
            self.spawn_workers()?;
        }
        Ok(())
    }

    /// Run the lanes of every lanes stage on threads of their own, from
    /// the next [`Rack::prepare`]: the first lane on the thread that calls
    /// `process`, every other on a worker that waits for each block, so a
    /// block takes as long as the longest lane rather than all of them.
    /// The result is bit for bit the sequential one, in the same mix order.
    ///
    /// `hook` runs once at the start of every worker thread, for the host
    /// to raise its priority to the audio thread's; a worker at ordinary
    /// priority can be held off the core when the machine is busy, and the
    /// audio thread waits for it. Workers spin briefly for the next block
    /// and then park, and are woken without allocating.
    pub fn set_parallel_lanes(&mut self, on: bool, hook: Option<ThreadHook>) {
        self.parallel = on;
        self.hook = Hook(hook);
    }

    /// Whether lanes run on threads of their own now.
    pub fn parallel_lanes(&self) -> bool {
        !self.workers.is_empty()
    }

    /// One worker per lane after the first of every lanes stage, in stage
    /// then lane order. `process` walks the stages in that same order, so
    /// the nth worker it meets is `workers[n]`, and waking one costs an
    /// `unpark` and no lock.
    fn spawn_workers(&mut self) -> Result<()> {
        for stage in &self.stages {
            if let Stage::Lanes(lanes) = stage {
                for cell in lanes.iter().skip(1) {
                    // SAFETY: `&mut self`, and no worker is running yet.
                    let lane = unsafe { cell.lane_held() }.name.clone();
                    let handle = thread::Builder::new()
                        .name(format!("lane {lane}"))
                        .spawn({
                            let cell = cell.clone();
                            let hook = self.hook.clone();
                            move || worker(cell, hook)
                        })
                        .map_err(|source| Error::Thread { lane, source })?;
                    self.workers.push(handle);
                }
            }
        }
        Ok(())
    }

    fn stop_workers(&mut self) {
        if self.workers.is_empty() {
            return;
        }
        for stage in &self.stages {
            if let Stage::Lanes(lanes) = stage {
                for cell in lanes.iter().skip(1) {
                    cell.state.store(QUIT, Ordering::Release);
                }
            }
        }
        for handle in &self.workers {
            handle.thread().unpark();
        }
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
        for stage in &self.stages {
            if let Stage::Lanes(lanes) = stage {
                for cell in lanes {
                    cell.state.store(IDLE, Ordering::Release);
                }
            }
        }
    }

    /// Forget the signal in every module, keeping every buffer. Does not
    /// allocate, but a capture settles itself on silence, which takes as
    /// long as running that much audio; keep it off the audio thread.
    pub fn reset(&mut self) {
        self.faulted.store(false, Ordering::Relaxed);
        for stage in &mut self.stages {
            match stage {
                Stage::Module(m) => m.node.reset(),
                Stage::Lanes(lanes) => lanes.iter().for_each(|cell| {
                    // SAFETY: `&mut self`, so no worker is between GO and DONE.
                    unsafe { cell.lane_held() }.reset()
                }),
            }
        }
    }

    /// Whether a block has come out non-finite since the rack was last
    /// [reset](Rack::reset).
    ///
    /// One `NaN` reaching a feedback path stays there and poisons every
    /// block after it: a reverb's recirculation, a delay line, a capture's
    /// recurrent state. `process` therefore checks what it produced,
    /// and on anything not finite it silences the block, clears what it can
    /// clear inside a callback, and sets this. Everything but a capture
    /// recovers on the spot; a capture needs [`Rack::reset`], which is the
    /// host's to call from the thread that prepared the rack.
    pub fn faulted(&self) -> bool {
        self.faulted.load(Ordering::Relaxed)
    }

    /// Drop the signal held in every module, as far as that can be done
    /// without taking real time. Used by `process` to recover from a
    /// non-finite block.
    fn silence(&mut self) {
        for stage in &mut self.stages {
            match stage {
                Stage::Module(m) => m.node.silence(),
                Stage::Lanes(lanes) => lanes.iter().for_each(|cell| {
                    // SAFETY: `&mut self`, and every worker was joined
                    // before `run_block` returned.
                    unsafe { cell.lane_held() }.silence()
                }),
            }
        }
        for bus in &mut self.buses {
            bus.frames = 0;
            for channel in &mut bus.data {
                channel.fill(0.0);
            }
        }
    }

    /// The rate the rack was prepared at, or `None` before the first
    /// [`Rack::prepare`].
    pub fn sample_rate(&self) -> Option<f64> {
        self.prepared.map(|p| p.sample_rate)
    }

    /// The largest block the rack was prepared for, or `None` before the
    /// first [`Rack::prepare`]. A larger block handed to [`Rack::process`]
    /// is not refused: it is split into blocks of at most this many frames.
    pub fn max_block(&self) -> Option<usize> {
        self.prepared.map(|p| p.max_block)
    }

    /// Output channels: 2 once the rack has a lanes stage, else 1.
    pub fn channels_out(&self) -> usize {
        self.channels_out
    }

    /// The delay the rack adds, in frames: the sum over the stages, a lanes
    /// stage counting as its longest lane, every module counted whether or
    /// not it is bypassed, so that it does not change under a host's feet.
    /// The shorter lanes of a stage are padded up to the longest at
    /// `prepare`, so this is what every path through the rack takes.
    pub fn latency(&self) -> usize {
        self.stages
            .iter()
            .map(|s| match s {
                Stage::Module(m) => m.node.latency(),
                Stage::Lanes(lanes) => lanes
                    .iter()
                    .map(|cell| cell.latency.load(Ordering::Relaxed))
                    .max()
                    .unwrap_or(0),
            })
            .sum()
    }

    /// Run a block: a mono `input`, and one `output` slice per
    /// [`Rack::channels_out`], all of one length. Allocation-free,
    /// lock-free, and a pure function of the buffers and the rack's state,
    /// so the output does not depend on how frames are grouped.
    ///
    /// Any number of frames is accepted, whatever the rack was prepared
    /// for: more than `max_block` is split into blocks of at most that many
    /// and run in turn, because a host such as CoreAudio may deliver more
    /// frames than it asked for, and an audio callback is no place to find
    /// that out. Zero frames does nothing.
    ///
    /// # Panics
    ///
    /// If the rack has not been [prepared](Rack::prepare), if `output` does
    /// not hold exactly [`Rack::channels_out`] slices, or if any of them is
    /// a different length from `input`. All three are caller mistakes that
    /// would otherwise show up as silence or as a slice index, and none
    /// depends on the audio.
    pub fn process(&mut self, input: &[f32], output: &mut [&mut [f32]]) {
        let frames = input.len();
        assert_eq!(
            output.len(),
            self.channels_out,
            "one output slice per channel"
        );
        assert!(
            output.iter().all(|o| o.len() == frames),
            "input and outputs must be the same length"
        );
        // The host's thread, and its floating-point mode, are put back as
        // they were found when this returns.
        let _flushing = Restoring::arm();
        let most = self
            .prepared
            .expect("the rack must be prepared before it is run")
            .max_block;
        let mut pos = 0;
        while pos < frames {
            let n = (frames - pos).min(most);
            let block = &input[pos..pos + n];
            match output {
                [left] => self.run_block(block, &mut [&mut left[pos..pos + n]]),
                [left, right] => self.run_block(
                    block,
                    &mut [&mut left[pos..pos + n], &mut right[pos..pos + n]],
                ),
                _ => unreachable!("channels_out is 1 or 2"),
            }
            // A NaN or an infinity in a feedback path never leaves on its
            // own, so it is stopped here rather than sent on.
            if output
                .iter()
                .any(|o| o[pos..pos + n].iter().any(|v| !v.is_finite()))
            {
                self.faulted.store(true, Ordering::Relaxed);
                self.silence();
                for o in output.iter_mut() {
                    o[pos..pos + n].fill(0.0);
                }
            }
            pos += n;
        }
    }

    /// One block of at most `max_block` frames, the whole of it through
    /// every stage.
    fn run_block(&mut self, input: &[f32], output: &mut [&mut [f32]]) {
        let n = input.len();
        let Rack {
            stages,
            a,
            b,
            buses,
            workers,
            ..
        } = self;
        let parallel = !workers.is_empty();
        // Workers were spawned in stage then lane order, which is the order
        // the loop below meets them.
        let mut worker_index = 0usize;
        // A bus carries what was sent on it this block, and nothing older.
        for bus in buses.iter_mut() {
            bus.frames = 0;
        }
        // Ping-pong between the two scratch pairs. The copies in and out cost
        // a block each, which is nothing next to a module; they buy a loop
        // with no special cases for the first and last stage.
        a[0][..n].copy_from_slice(input);
        let mut width = 1;
        let mut in_a = true;
        for stage in stages.iter_mut() {
            let (src, dst) = if in_a { (&*a, &mut *b) } else { (&*b, &mut *a) };
            match stage {
                Stage::Module(m) => {
                    m.ran = !m.bypass.load(Ordering::Relaxed);
                    if !m.ran {
                        continue;
                    }
                    if let Some(bus) = m.node.receives() {
                        buses[bus.min(BUSES - 1)].hand_to(m.node.as_mut());
                    }
                    run_module(m.node.as_mut(), src, dst, width, n);
                    if let Some((bus, block)) = m.node.sends() {
                        buses[bus.min(BUSES - 1)].take(block);
                    }
                }
                Stage::Lanes(lanes) => {
                    dst[0][..n].fill(0.0);
                    dst[1][..n].fill(0.0);
                    let s: [&[f32]; 2] = [&src[0][..n], &src[1][..n]];
                    // The returns in the lanes hear the buses as they stand
                    // now, before any lane runs.
                    for cell in lanes.iter() {
                        // SAFETY: `&mut self`, and no lane of this stage has
                        // been dispatched yet, so no worker holds one.
                        for m in &mut unsafe { cell.lane_held() }.modules {
                            if let Some(bus) = m.node.receives() {
                                buses[bus.min(BUSES - 1)].hand_to(m.node.as_mut());
                            }
                        }
                    }
                    if parallel {
                        // Hand every lane but the first to its worker, then
                        // run the first here while they work.
                        for cell in lanes.iter().skip(1) {
                            // SAFETY: this lane has not been dispatched this
                            // block, and the last one was joined before
                            // `process` returned, so no worker holds it.
                            let lane = unsafe { cell.lane_held() };
                            if lane.bypass.load(Ordering::Relaxed) {
                                lane.ran = false;
                                worker_index += 1;
                                continue;
                            }
                            cell.src[0].store(s[0].as_ptr().cast_mut(), Ordering::Relaxed);
                            cell.src[1].store(s[1].as_ptr().cast_mut(), Ordering::Relaxed);
                            cell.n.store(n, Ordering::Relaxed);
                            cell.state.store(GO, Ordering::Release);
                            workers[worker_index].thread().unpark();
                            worker_index += 1;
                        }
                    }
                    for (i, cell) in lanes.iter().enumerate() {
                        if parallel && i > 0 {
                            // Not dispatched: it was bypassed.
                            if cell.state.load(Ordering::Acquire) == IDLE {
                                continue;
                            }
                            while cell.state.load(Ordering::Acquire) != DONE {
                                std::hint::spin_loop();
                            }
                            cell.state.store(IDLE, Ordering::Relaxed);
                            let in_lane_a = cell.in_a.load(Ordering::Relaxed);
                            // SAFETY: the worker stored DONE, which this
                            // thread acquired, so the lane is back.
                            unsafe { cell.lane_held() }.mix_into(in_lane_a, dst, n);
                        } else {
                            // SAFETY: the first lane always runs here.
                            let lane = unsafe { cell.lane_held() };
                            if lane.bypass.load(Ordering::Relaxed) {
                                lane.ran = false;
                                continue;
                            }
                            let in_lane_a = lane.run(s, n);
                            lane.mix_into(in_lane_a, dst, n);
                        }
                    }
                    // Every worker has been joined: collect the sends.
                    for cell in lanes.iter() {
                        // SAFETY: every lane of this stage is back.
                        let lane = unsafe { cell.lane_held() };
                        if !lane.ran {
                            continue;
                        }
                        for m in &lane.modules {
                            if m.ran
                                && let Some((bus, block)) = m.node.sends()
                            {
                                buses[bus.min(BUSES - 1)].take(block);
                            }
                        }
                    }
                    width = 2;
                }
            }
            in_a = !in_a;
        }
        let result = if in_a { &*a } else { &*b };
        for (c, out) in output.iter_mut().enumerate() {
            out.copy_from_slice(&result[c][..n]);
        }
        // The taps: every module that ran, in rack order, adds what it has
        // for the output. Every worker has been joined by now.
        for stage in stages.iter_mut() {
            match stage {
                Stage::Module(m) => {
                    if m.ran {
                        m.node.tap(output);
                    }
                }
                Stage::Lanes(lanes) => {
                    for cell in lanes.iter() {
                        // SAFETY: every worker was joined before the taps.
                        let lane = unsafe { cell.lane_held() };
                        if !lane.ran {
                            continue;
                        }
                        for m in &mut lane.modules {
                            if m.ran {
                                m.node.tap(output);
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Drop for Rack {
    fn drop(&mut self) {
        self.stop_workers();
    }
}

#[derive(Debug)]
struct Entry {
    name: String,
    type_id: &'static str,
    bypass: Arc<AtomicBool>,
    params: Vec<Arc<Param>>,
}

impl Entry {
    fn module(m: &Module) -> Self {
        Self {
            name: m.name.clone(),
            type_id: m.node.type_id(),
            bypass: m.bypass.clone(),
            params: m.node.params().to_vec(),
        }
    }
}

/// A handle on every parameter and bypass of a rack, usable from any thread
/// while the rack runs on the audio one.
///
/// Addresses are `name/parameter`, the name being a module's or a lane's:
/// `output/level_db`, `A/pan`, `A.capture` has none. Every set is a clamped
/// atomic store; the module reads it on its next block.
#[derive(Debug, Clone)]
pub struct Controls {
    inner: Arc<Vec<Entry>>,
}

impl Controls {
    fn entry(&self, name: &str) -> Result<&Entry> {
        self.inner
            .iter()
            .find(|e| e.name == name)
            .ok_or_else(|| Error::UnknownName(name.into()))
    }

    fn find(&self, address: &str) -> Result<&Arc<Param>> {
        let (name, id) = address
            .split_once('/')
            .ok_or_else(|| Error::BadAddress(address.into()))?;
        self.entry(name)?
            .params
            .iter()
            .find(|p| p.spec().id == id)
            .ok_or_else(|| Error::UnknownParam(address.into()))
    }

    /// Set a parameter, clamped to its range.
    pub fn set(&self, address: &str, value: f32) -> Result<()> {
        self.find(address)?.set(value);
        Ok(())
    }

    /// A parameter's current value.
    pub fn get(&self, address: &str) -> Result<f32> {
        Ok(self.find(address)?.get())
    }

    /// The parameter itself, for a control that changes it often: no lookup
    /// per change.
    pub fn param(&self, address: &str) -> Result<Arc<Param>> {
        self.find(address).cloned()
    }

    /// Take a module out of the signal path, or a lane out of the mix, or
    /// put it back.
    pub fn set_bypass(&self, name: &str, on: bool) -> Result<()> {
        self.entry(name)?.bypass.store(on, Ordering::Relaxed);
        Ok(())
    }

    /// Whether a module or lane is bypassed.
    pub fn bypassed(&self, name: &str) -> Result<bool> {
        Ok(self.entry(name)?.bypass.load(Ordering::Relaxed))
    }

    /// Every module and lane: name and type id, in rack order, a lane before
    /// its modules.
    pub fn modules(&self) -> Vec<(String, &'static str)> {
        self.inner
            .iter()
            .map(|e| (e.name.clone(), e.type_id))
            .collect()
    }

    /// Every parameter: address, description and current value, in order.
    pub fn params(&self) -> Vec<(String, ParamSpec, f32)> {
        self.inner
            .iter()
            .flat_map(|e| {
                e.params
                    .iter()
                    .map(move |p| (format!("{}/{}", e.name, p.spec().id), *p.spec(), p.get()))
            })
            .collect()
    }
}
