//! A rack from a `.rig` preset, and a running rack written back into one.
//!
//! The preset names modules by type and assets by content. This module is
//! the registry that turns `std.*` types into nodes; the resolver the host
//! supplies turns content addresses into loaded assets, and owns where the
//! bytes come from and checking their hash. The format's checks are shape
//! only: the values are applied through [`Controls`], so a parameter the
//! module does not have is refused here by address, a value out of range is
//! clamped to the descriptor as any set is, and a type the engine does not
//! have is refused by name.

use crate::bus::{BusReturn, BusSend};
use crate::cabinet::Cabinet;
use crate::capture::Capture;
use crate::chorus::Chorus;
use crate::compressor::Compressor;
use crate::delay::Delay;
use crate::drive::Drive;
use crate::eq::Tone;
use crate::gain::Gain;
use crate::gate::Gate;
use crate::phaser::Phaser;
use crate::rack::{Append, Controls, Lane, Rack};
use crate::reverb::Reverb;
use crate::tap::Tap;
use crate::{Error, Result};
use valverig_format::{Asset, AssetKind, Module, Preset, Stage};
use valverig_nam::loader::Model;

/// An asset, loaded: what a resolver hands back for a label.
#[derive(Debug)]
pub enum Loaded {
    /// A capture, parsed.
    Capture(Model),
    /// An impulse response as mono samples, and the rate it was recorded
    /// at when known.
    Ir {
        /// The samples.
        samples: Vec<f32>,
        /// The rate, if known; the cabinet refuses a mismatch at `prepare`.
        sample_rate: Option<f64>,
    },
}

/// What a resolver returns when it cannot load an asset.
pub type ResolveError = Box<dyn std::error::Error + Send + Sync>;

/// Turns an asset into something loaded, given its label and its entry.
/// The host owns where bytes come from and checking that they hash to the
/// address.
pub type Resolver<'a> = dyn FnMut(&str, &Asset) -> std::result::Result<Loaded, ResolveError> + 'a;

/// What a host can ask of a build beyond the preset itself.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BuildOptions {
    /// Bring every capture that states its loudness to this level, in
    /// dBFS; the reference plugin aims at -18. `None` leaves them as they
    /// are.
    pub normalize_captures_to_dbfs: Option<f32>,
}

/// Build the preset's rack. Every `std.*` type becomes its node, captures
/// and cabinets load their asset through `resolve`, and then every value
/// and bypass in the preset is applied through the controls.
///
/// Pass [`BuildOptions::default()`] to take the preset exactly as it
/// stands.
pub fn build(
    preset: &Preset,
    resolve: &mut Resolver<'_>,
    options: BuildOptions,
) -> Result<(Rack, Controls)> {
    preset.validate()?;
    let mut builder = Rack::builder();
    for stage in &preset.stages {
        builder = match stage {
            Stage::Module(m) => append(builder, m, preset, resolve, options)?,
            Stage::Lanes(lanes) => {
                let mut built = Vec::with_capacity(lanes.len());
                for l in lanes {
                    let mut lane = Lane::new(&l.name)
                        .pan(l.pan as f32)
                        .level_db(l.level_db as f32);
                    for m in &l.modules {
                        lane = append(lane, m, preset, resolve, options)?;
                    }
                    built.push(lane);
                }
                builder.split(built)?
            }
        };
    }
    let (rack, controls) = builder.build();
    for (address, value) in preset.values() {
        controls.set(&address, value as f32)?;
    }
    for (name, on) in preset.bypasses() {
        controls.set_bypass(&name, on)?;
    }
    Ok((rack, controls))
}

/// Write the rack's state into the preset: every parameter's current value
/// and every module's and lane's bypass. The preset must be the one the
/// rack was built from, or one with the same names.
pub fn snapshot(preset: &mut Preset, controls: &Controls) -> Result<()> {
    for (address, _, value) in controls.params() {
        preset.set_value(&address, value as f64)?;
    }
    for (name, _) in controls.modules() {
        preset.set_bypass(&name, controls.bypassed(&name)?)?;
    }
    Ok(())
}

/// The registry: append the module `m` asks for to `target`, which is the
/// rack under construction or one of its lanes. Each arm names a concrete
/// type, so nothing here is boxed twice or dispatched to find out what it
/// is.
fn append<T: Append>(
    target: T,
    m: &Module,
    preset: &Preset,
    resolve: &mut Resolver<'_>,
    options: BuildOptions,
) -> Result<T> {
    let name = m.name.as_str();
    Ok(match m.type_id.as_str() {
        Gain::TYPE_ID => target.append(name, Gain::new())?,
        Gate::TYPE_ID => target.append(name, Gate::new())?,
        Compressor::TYPE_ID => target.append(name, Compressor::new())?,
        Tone::TYPE_ID => target.append(name, Tone::new())?,
        Reverb::TYPE_ID => target.append(name, Reverb::new())?,
        Drive::TYPE_ID => target.append(name, Drive::new())?,
        Chorus::TYPE_ID => target.append(name, Chorus::new())?,
        Delay::TYPE_ID => target.append(name, Delay::new())?,
        Phaser::TYPE_ID => target.append(name, Phaser::new())?,
        Tap::TYPE_ID => target.append(name, Tap::new())?,
        BusSend::TYPE_ID => target.append(name, BusSend::new())?,
        BusReturn::TYPE_ID => target.append(name, BusReturn::new())?,
        Capture::TYPE_ID => {
            let (label, loaded) = load(m, preset, resolve)?;
            match loaded {
                Loaded::Capture(model) => {
                    let capture = Capture::new(model);
                    let capture = match options.normalize_captures_to_dbfs {
                        Some(target_dbfs) => capture.normalized(target_dbfs),
                        None => capture,
                    };
                    target.append(name, capture)?
                }
                Loaded::Ir { .. } => {
                    return Err(wrong_kind(m, label, AssetKind::Nam, &loaded));
                }
            }
        }
        Cabinet::TYPE_ID => {
            let (label, loaded) = load(m, preset, resolve)?;
            match loaded {
                Loaded::Ir {
                    samples,
                    sample_rate,
                } => target.append(name, Cabinet::new(&samples, sample_rate))?,
                Loaded::Capture(_) => {
                    return Err(wrong_kind(m, label, AssetKind::Ir, &loaded));
                }
            }
        }
        _ => {
            return Err(Error::UnknownType {
                module: m.name.clone(),
                type_id: m.type_id.clone(),
            });
        }
    })
}

/// The asset the module names, loaded, and the label it was named by.
fn load<'a>(
    m: &'a Module,
    preset: &Preset,
    resolve: &mut Resolver<'_>,
) -> Result<(&'a str, Loaded)> {
    let label = m.asset.as_deref().ok_or_else(|| Error::MissingAsset {
        module: m.name.clone(),
    })?;
    let asset = preset
        .assets
        .get(label)
        .ok_or_else(|| valverig_format::Error::UnknownAsset {
            module: m.name.clone(),
            asset: label.into(),
        })?;
    let loaded = resolve(label, asset).map_err(|source| Error::Resolve {
        module: m.name.clone(),
        asset: label.into(),
        source,
    })?;
    Ok((label, loaded))
}

fn wrong_kind(m: &Module, asset: &str, needs: AssetKind, found: &Loaded) -> Error {
    Error::WrongAssetKind {
        module: m.name.clone(),
        asset: asset.into(),
        needs,
        found: match found {
            Loaded::Capture(_) => AssetKind::Nam,
            Loaded::Ir { .. } => AssetKind::Ir,
        },
    }
}
