//! The `.rig` preset format, and the rack built from one.
//!
//! A preset says what to load and how the rack is configured, and nothing the
//! engine could not get from the assets it names. It is TOML, written to be
//! read and edited by hand.
//!
//! ```
//! use valrig_core::preset::{Preset, Stage};
//!
//! let mut preset = Preset::parse(include_str!("../../assets/plexi-tweed-wide.rig"))?;
//!
//! // Applying a preset is a walk over its addressed values.
//! for (address, value) in preset.values() {
//!     let _ = (address, value); // "A.eq/bass_db", 1.5
//! }
//!
//! // A host writes a running rack back the same way, then saves it.
//! preset.set_value("gate/threshold_db", -40.0)?;
//! preset.set_bypass("reverb", true)?;
//! let text = preset.to_toml()?;
//! assert_eq!(Preset::parse(&text)?, preset);
//! # Ok::<(), valrig_core::Error>(())
//! ```
//!
//! # The document
//!
//! ```toml
//! format_version = 1
//! name = "Plexi + Tweed · Wide"
//!
//! [assets.plexi]                    # what to load, by content, not by path
//! kind = "nam"                      # "nam" or "ir"
//! hash = "blake3:…"                 # 64 hex digits of the BLAKE3 of the bytes
//! file = "brit-plexi-50-v2.nam"     # a file name, to help recognise it
//!
//! [[stage]]                         # stages in series; this one is a module
//! type = "std.gate"
//! name = "gate"
//! [stage.params]
//! threshold_db = -50.0
//!
//! [[stage]]                         # this one is lanes in parallel
//! [[stage.lane]]
//! name = "A"
//! pan = -1.0
//! [[stage.lane.module]]
//! type = "std.capture"
//! name = "A.capture"
//! asset = "plexi"
//! ```
//!
//! A [`Stage`] is either a [`Module`], which has a namespaced `type`, a
//! `name`, an optional `asset`, `bypass`, `params` and a `ui` table of
//! presentation hints the engine ignores, or [`Lane`]s in parallel, each with
//! a `name`, `level_db`, `pan`, `bypass` and its own modules. Names are unique across the whole
//! preset and form the first half of every address, `name/parameter`, which
//! is how the rack's [`Controls`](crate::rack::Controls) are addressed too.
//! Parameter ids are stable strings, never indices.
//!
//! # What is checked, and where
//!
//! [`Preset::validate`] checks shape: the version, that names are unique and
//! can be addressed, that types are namespaced, that every `asset` a module
//! names is carried, that hashes and file names are well formed, and that
//! every number is finite. A document that gets any of it wrong is refused
//! with an [`Error`] that names the part at fault.
//!
//! The document does not know which module types exist, what parameters they
//! have, or what ranges those take. [`build`] does: it turns each `std.*` type
//! into its module, refuses a type or a parameter the engine does not have by
//! name, and applies every value through the controls, which clamp it to the
//! parameter's descriptor like any other set. The document never becomes a
//! second copy of the descriptors. [`snapshot`] writes a running rack's state
//! back into the preset it was built from.

mod asset;
mod document;
mod hash;
mod registry;
pub(crate) mod text;

pub use asset::{Asset, AssetKind};
pub use hash::Hash;
pub use registry::{BuildOptions, Loaded, ResolveError, Resolver, build, snapshot};

use crate::error::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

/// The content address of an asset: the BLAKE3 hash of its bytes.
///
/// This is what a preset's `hash` field carries, and what a resolver compares
/// against to know it found the right file.
///
/// ```
/// use valrig_core::preset::asset_hash;
///
/// assert_eq!(
///     asset_hash(b"hello").as_str(),
///     "blake3:ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f",
/// );
/// ```
pub fn asset_hash(bytes: &[u8]) -> Hash {
    Hash::of(bytes)
}

/// The only format version this crate reads and writes.
pub const FORMAT_VERSION: u32 = 1;

/// The most assets one preset may carry.
pub const MAX_ASSETS: usize = 256;

/// The most stages one preset may have.
pub const MAX_STAGES: usize = 256;

/// The most lanes one parallel stage may have.
pub const MAX_LANES: usize = 64;

/// The most modules one preset may have, counting those inside lanes.
///
/// This is the count that matters: a reader allocates per module, and a
/// reverb or a cabinet allocates a great deal each.
pub const MAX_MODULES: usize = 1024;

/// One module: a namespaced type, a name, and its settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// The namespaced type the engine builds from, `std.gate`.
    pub type_id: String,
    /// What this module is called. Unique across the whole preset, and the
    /// first half of every address into it.
    pub name: String,
    /// The label of the asset it loads, for a capture or a cabinet.
    pub asset: Option<String>,
    /// Out of the signal path, passing its input through unchanged.
    pub bypass: bool,
    /// Parameter values by stable id. A parameter the preset does not set is
    /// left at the engine's default for that module type.
    pub params: BTreeMap<String, f64>,
    /// Presentation hints the engine ignores: the `label` a person gave this
    /// module, and whatever else an interface wants to remember. Keys are
    /// identifiers; values are free text.
    pub ui: BTreeMap<String, String>,
}

impl Module {
    /// Check everything about this module except its name, which the preset
    /// checks so it can catch duplicates across stages.
    fn validate(&self, assets: &BTreeMap<String, Asset>) -> Result<()> {
        if !text::is_type_id(&self.type_id) {
            return Err(Error::BadType(self.type_id.clone()));
        }
        if let Some(label) = &self.asset
            && !assets.contains_key(label)
        {
            return Err(Error::UnknownAsset {
                module: self.name.clone(),
                asset: label.clone(),
            });
        }
        for (id, value) in &self.params {
            if !text::is_identifier(id) {
                return Err(Error::BadParamId {
                    module: self.name.clone(),
                    id: id.clone(),
                });
            }
            if !value.is_finite() {
                return Err(Error::NotFinite(format!("{}/{id}", self.name)));
            }
        }
        for key in self.ui.keys() {
            if !text::is_identifier(key) {
                return Err(Error::BadUiKey {
                    module: self.name.clone(),
                    key: key.clone(),
                });
            }
        }
        Ok(())
    }
}

/// One lane of a parallel stage: modules in series, then a level and a pan
/// into the mix.
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    /// What this lane is called. Unique across the whole preset alongside
    /// module names, and addressable the same way.
    pub name: String,
    /// Into the mix, in dB. `0.0` is unity.
    pub level_db: f64,
    /// Across the mix: `-1.0` hard left, `0.0` centre, `1.0` hard right.
    /// Values outside that range reach the engine, which clamps them to what
    /// its descriptor allows.
    pub pan: f64,
    /// Out of the mix entirely.
    pub bypass: bool,
    /// The modules, in series.
    pub modules: Vec<Module>,
}

impl Lane {
    /// Check this lane's own numbers. Its name and its modules are the
    /// preset's business.
    fn validate(&self) -> Result<()> {
        for (field, value) in [("level_db", self.level_db), ("pan", self.pan)] {
            if !value.is_finite() {
                return Err(Error::NotFinite(format!("{}/{field}", self.name)));
            }
        }
        Ok(())
    }
}

/// One stage of the rack: everything in a stage happens before the next one
/// begins.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    /// One module, in series with the stages either side of it.
    Module(Module),
    /// Lanes in parallel, summed into the next stage. Never empty.
    Lanes(Vec<Lane>),
}

/// A preset: the rack, and the assets it names.
///
/// This is plain data. Build one in code and [write](Self::to_toml) it, or
/// [read](Self::parse) one and walk it. Nothing here loads anything or makes
/// a sound.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    /// The format version. Always [`FORMAT_VERSION`]; a document written to
    /// any other version is refused.
    pub format_version: u32,
    /// A title for people. The format does not interpret it.
    pub name: String,
    /// Who made it.
    pub author: Option<String>,
    /// Anything worth saying about it.
    pub notes: Option<String>,
    /// The assets, under labels local to this preset.
    pub assets: BTreeMap<String, Asset>,
    /// The rack, in order.
    pub stages: Vec<Stage>,
}

impl Preset {
    /// An empty preset at the current format version.
    pub fn new(name: &str) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            name: name.to_string(),
            author: None,
            notes: None,
            assets: BTreeMap::new(),
            stages: Vec::new(),
        }
    }

    /// Check the whole document's shape.
    ///
    /// This is what [`parse`](Self::parse) runs after reading and
    /// [`to_toml`](Self::to_toml) runs before writing, so a preset that came
    /// from this crate can always be written, and anything written can always
    /// be read back.
    ///
    /// It checks shape, never meaning: which module types exist and what
    /// parameters they have belongs to the engine's registry, which says so
    /// when it applies the values.
    ///
    /// # Errors
    ///
    /// The first thing wrong, as the [`Error`] naming it.
    pub fn validate(&self) -> Result<()> {
        check_version(self.format_version)?;
        self.check_sizes()?;
        for (label, asset) in &self.assets {
            asset.validate(label)?;
        }
        let mut claimed: BTreeSet<&str> = BTreeSet::new();
        for (index, stage) in self.stages.iter().enumerate() {
            match stage {
                Stage::Module(m) => {
                    claim(&mut claimed, &m.name)?;
                    m.validate(&self.assets)?;
                }
                Stage::Lanes(lanes) => {
                    if lanes.is_empty() {
                        return Err(Error::Shape(format!(
                            "stage {index}: a parallel stage needs at least one lane"
                        )));
                    }
                    for lane in lanes {
                        claim(&mut claimed, &lane.name)?;
                        lane.validate()?;
                        for m in &lane.modules {
                            claim(&mut claimed, &m.name)?;
                            m.validate(&self.assets)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Refuse a preset that declares more than a reader will build.
    ///
    /// Whoever turns this document into a rack allocates per stage, per
    /// lane and per module, so an unbounded count here is an unbounded
    /// allocation there. The limits are far above any rig a person would
    /// build and far below anything that would exhaust memory.
    fn check_sizes(&self) -> Result<()> {
        let too_many = |what, found: usize, max| {
            if found > max {
                Err(Error::TooMany { what, found, max })
            } else {
                Ok(())
            }
        };
        too_many("assets", self.assets.len(), MAX_ASSETS)?;
        too_many("stages", self.stages.len(), MAX_STAGES)?;
        let mut modules = 0;
        for stage in &self.stages {
            match stage {
                Stage::Module(_) => modules += 1,
                Stage::Lanes(lanes) => {
                    too_many("lanes in one stage", lanes.len(), MAX_LANES)?;
                    modules += lanes.iter().map(|l| l.modules.len()).sum::<usize>();
                }
            }
        }
        too_many("modules", modules, MAX_MODULES)
    }

    /// Every module, in rack order, a lane's modules where that lane sits.
    pub fn modules(&self) -> impl Iterator<Item = &Module> {
        self.stages.iter().flat_map(|stage| {
            let (module, lanes) = match stage {
                Stage::Module(m) => (Some(m), None),
                Stage::Lanes(lanes) => (None, Some(lanes)),
            };
            module.into_iter().chain(
                lanes
                    .into_iter()
                    .flatten()
                    .flat_map(|lane| lane.modules.iter()),
            )
        })
    }

    /// Every value the preset sets, as `name/parameter`, in rack order: a
    /// lane's `level_db` and `pan` before its modules' parameters.
    ///
    /// These are the addresses the engine's controls take, so applying a
    /// preset is a walk over this list. A parameter the preset does not set
    /// does not appear; a lane's level and pan always do, because a lane
    /// always has them.
    pub fn values(&self) -> Vec<(String, f64)> {
        let mut out = Vec::new();
        for stage in &self.stages {
            match stage {
                Stage::Module(m) => push_params(&mut out, m),
                Stage::Lanes(lanes) => {
                    for lane in lanes {
                        out.push((format!("{}/level_db", lane.name), lane.level_db));
                        out.push((format!("{}/pan", lane.name), lane.pan));
                        for m in &lane.modules {
                            push_params(&mut out, m);
                        }
                    }
                }
            }
        }
        out
    }

    /// Every module and lane with whether it is bypassed, in rack order.
    pub fn bypasses(&self) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        for stage in &self.stages {
            match stage {
                Stage::Module(m) => out.push((m.name.clone(), m.bypass)),
                Stage::Lanes(lanes) => {
                    for lane in lanes {
                        out.push((lane.name.clone(), lane.bypass));
                        out.extend(lane.modules.iter().map(|m| (m.name.clone(), m.bypass)));
                    }
                }
            }
        }
        out
    }

    /// Set a value by address, `name/parameter`: a module's parameter, or a
    /// lane's `level_db` or `pan`.
    ///
    /// This is how a host writes a running rack back into a preset. A
    /// parameter the module did not have is added; the engine says whether it
    /// means anything when the preset is applied.
    ///
    /// # Errors
    ///
    /// [`Error::NotFinite`] if `value` is `nan` or an infinity;
    /// [`Error::Shape`] if `address` has no `/`, or names something other
    /// than `level_db` or `pan` on a lane; [`Error::UnknownName`] if no
    /// module or lane goes by that name.
    pub fn set_value(&mut self, address: &str, value: f64) -> Result<()> {
        if !value.is_finite() {
            return Err(Error::NotFinite(address.to_string()));
        }
        let (name, id) = address.split_once('/').ok_or_else(|| {
            Error::Shape(format!(
                "{address:?} is not an address: expected name/parameter"
            ))
        })?;
        if let Some(m) = self.module_mut(name) {
            m.params.insert(id.to_string(), value);
            return Ok(());
        }
        let Some(lane) = self.lane_mut(name) else {
            return Err(Error::UnknownName(name.to_string()));
        };
        match id {
            "level_db" => lane.level_db = value,
            "pan" => lane.pan = value,
            other => {
                return Err(Error::Shape(format!(
                    "a lane has level_db and pan, not {other:?}"
                )));
            }
        }
        Ok(())
    }

    /// Take a module or lane out of the signal path, or put it back.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownName`] if no module or lane goes by that name.
    pub fn set_bypass(&mut self, name: &str, on: bool) -> Result<()> {
        if let Some(m) = self.module_mut(name) {
            m.bypass = on;
            return Ok(());
        }
        if let Some(lane) = self.lane_mut(name) {
            lane.bypass = on;
            return Ok(());
        }
        Err(Error::UnknownName(name.to_string()))
    }

    fn module_mut(&mut self, name: &str) -> Option<&mut Module> {
        self.stages.iter_mut().find_map(|stage| match stage {
            Stage::Module(m) if m.name == name => Some(m),
            Stage::Lanes(lanes) => lanes
                .iter_mut()
                .flat_map(|l| l.modules.iter_mut())
                .find(|m| m.name == name),
            Stage::Module(_) => None,
        })
    }

    fn lane_mut(&mut self, name: &str) -> Option<&mut Lane> {
        self.stages.iter_mut().find_map(|stage| match stage {
            Stage::Lanes(lanes) => lanes.iter_mut().find(|l| l.name == name),
            Stage::Module(_) => None,
        })
    }
}

/// Refuse a version this crate does not read and write.
pub(crate) fn check_version(found: u32) -> Result<()> {
    if found == FORMAT_VERSION {
        Ok(())
    } else {
        Err(Error::Version {
            found,
            supported: FORMAT_VERSION,
        })
    }
}

/// Take a name for a module or lane, or say why it cannot be had.
fn claim<'a>(claimed: &mut BTreeSet<&'a str>, name: &'a str) -> Result<()> {
    if !text::is_name(name) {
        return Err(Error::BadName(name.to_string()));
    }
    if !claimed.insert(name) {
        return Err(Error::DuplicateName(name.to_string()));
    }
    Ok(())
}

fn push_params(out: &mut Vec<(String, f64)>, m: &Module) {
    out.extend(
        m.params
            .iter()
            .map(|(id, v)| (format!("{}/{id}", m.name), *v)),
    );
}
