//! The cabinet, the dsp module's [`Convolver`] and [`Cuts`], as a module:
//! one of each per channel, all of the same impulse response, under one
//! set of settings.

use std::sync::Arc;

use crate::dsp::convolution::Convolver;
use crate::dsp::cuts::{Cuts, HIGH_CUT_MIN_HZ, HIGH_CUT_OPEN_HZ, LOW_CUT_MAX_HZ, LOW_CUT_OPEN_HZ};

use crate::node::{Node, Param, ParamSpec, Unit};
use crate::{Error, Result};

/// The low cut's corner; at the bottom it is out of the path.
pub const LOW_CUT: ParamSpec = ParamSpec {
    id: "low_cut_hz",
    name: "Low cut",
    unit: Unit::Hertz,
    min: LOW_CUT_OPEN_HZ,
    max: LOW_CUT_MAX_HZ,
    default: LOW_CUT_OPEN_HZ,
};

/// The high cut's corner; at the top it is out of the path.
pub const HIGH_CUT: ParamSpec = ParamSpec {
    id: "high_cut_hz",
    name: "High cut",
    unit: Unit::Hertz,
    min: HIGH_CUT_MIN_HZ,
    max: HIGH_CUT_OPEN_HZ,
    default: HIGH_CUT_OPEN_HZ,
};

/// A cabinet impulse response in the rack, and the two cuts after it.
///
/// The host loads the IR and hands over its samples. When it also says what
/// rate the IR was recorded at, `prepare` at a different rate is an error,
/// because an IR played at the wrong rate is a different cabinet. With both
/// cuts parked at the open ends of their ranges the cabinet is the
/// convolution alone, bit for bit.
#[derive(Debug)]
pub struct Cabinet {
    /// The convolver as built, never run: the per-channel ones are clones.
    template: Convolver,
    ir_rate: Option<f64>,
    convolvers: Vec<Convolver>,
    cuts: Vec<Cuts>,
    params: [Arc<Param>; 2],
}

impl Cabinet {
    /// The type id.
    pub const TYPE_ID: &'static str = "std.cabinet";

    /// Build from an IR, and the rate it was recorded at when known.
    /// Allocates.
    pub fn new(ir: &[f32], ir_sample_rate: Option<f64>) -> Self {
        Self {
            template: Convolver::new(ir),
            ir_rate: ir_sample_rate,
            convolvers: Vec::new(),
            cuts: Vec::new(),
            params: [Param::new(LOW_CUT), Param::new(HIGH_CUT)],
        }
    }

    /// Push the parameters into every channel's cuts. Each setter is a
    /// comparison when nothing changed, so this runs every block.
    fn apply(&mut self) {
        let [low, high] = &self.params;
        for cuts in &mut self.cuts {
            cuts.set_low_cut_hz(low.get());
            cuts.set_high_cut_hz(high.get());
        }
    }
}

impl Node for Cabinet {
    fn type_id(&self) -> &'static str {
        Self::TYPE_ID
    }

    fn params(&self) -> &[Arc<Param>] {
        &self.params
    }

    fn prepare(&mut self, sample_rate: f64, _max_block: usize, channels: usize) -> Result<()> {
        if let Some(needs) = self.ir_rate
            && needs != sample_rate
        {
            return Err(Error::SampleRate {
                node: Self::TYPE_ID.into(),
                needs,
                got: sample_rate,
            });
        }
        self.convolvers = vec![self.template.clone(); channels];
        self.cuts = (0..channels).map(|_| Cuts::new(sample_rate)).collect();
        self.reset();
        Ok(())
    }

    fn reset(&mut self) {
        // Take the settings first, so that the cuts land on where the
        // parameters stand now rather than on where a glide was headed
        // when the signal was last running.
        self.apply();
        for convolver in &mut self.convolvers {
            convolver.reset();
        }
        for cuts in &mut self.cuts {
            cuts.reset();
        }
    }

    fn latency(&self) -> usize {
        self.template.latency()
    }

    fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        self.apply();
        for (((x, y), convolver), cuts) in input
            .iter()
            .zip(output.iter_mut())
            .zip(&mut self.convolvers)
            .zip(&mut self.cuts)
        {
            convolver.process(x, y);
            cuts.process(y);
        }
    }
}
