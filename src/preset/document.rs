//! The TOML the format is written as.
//!
//! The rest of the crate models a rack; this module models a file. They are
//! deliberately not the same shape. TOML has no tagged unions, so a stage is
//! written as one table that carries either a module's keys or an array of
//! lanes, and this module is where that flat table becomes a [`Stage`] and
//! back again.
//!
//! Every table refuses keys it does not know. A `.rig` is edited by hand and
//! re-saved by an application, so a key that was quietly ignored on the way
//! in would be quietly deleted on the way out: a typo would look like it
//! worked, and a field from a document this reader did not write would be
//! destroyed by opening and saving it. Refusing is the only behaviour that
//! keeps a document's meaning stable across a round trip.

use super::asset::{Asset, AssetKind};
use super::hash::Hash;
use super::{Lane, Module, Preset, Stage, check_version};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

impl Preset {
    /// Read a document and check its shape.
    ///
    /// # Errors
    ///
    /// [`Error::Toml`] if the text is not TOML or carries a key or type this
    /// format does not know; [`Error::Version`] if it was written to another
    /// version of the format; otherwise whatever
    /// [`validate`](Self::validate) finds.
    ///
    /// ```
    /// use valverig_core::preset::Preset;
    ///
    /// let preset = Preset::parse("
    ///     format_version = 1
    ///     name = \"Clean\"
    ///
    ///     [[stage]]
    ///     type = \"std.gain\"
    ///     name = \"input\"
    ///     [stage.params]
    ///     level_db = -3.0
    /// ")?;
    ///
    /// assert_eq!(preset.values(), [("input/level_db".to_string(), -3.0)]);
    /// # Ok::<(), valverig_core::Error>(())
    /// ```
    pub fn parse(text: &str) -> Result<Self> {
        let raw: Document = toml::from_str(text).map_err(|e| Error::Toml(e.to_string()))?;
        // Before the stages, so a document from a later version is told that
        // rather than being told its stages are the wrong shape.
        check_version(raw.format_version)?;

        let mut assets = BTreeMap::new();
        for (label, asset) in raw.assets {
            assets.insert(label, asset.into_asset()?);
        }
        let mut stages = Vec::with_capacity(raw.stages.len());
        for (index, stage) in raw.stages.into_iter().enumerate() {
            stages.push(stage.into_stage(index)?);
        }

        let preset = Self {
            format_version: raw.format_version,
            name: raw.name,
            author: raw.author,
            notes: raw.notes,
            assets,
            stages,
        };
        preset.validate()?;
        Ok(preset)
    }

    /// Write the document.
    ///
    /// Checks the shape first, so what comes out can always be read back in.
    /// Values left at their defaults are not written.
    ///
    /// # Errors
    ///
    /// Whatever [`validate`](Self::validate) finds, or [`Error::Write`] if
    /// the writer refuses the document.
    ///
    /// ```
    /// use valverig_core::preset::Preset;
    ///
    /// let preset = Preset::new("Clean");
    /// let text = preset.to_toml()?;
    /// assert_eq!(Preset::parse(&text)?, preset);
    /// # Ok::<(), valverig_core::Error>(())
    /// ```
    pub fn to_toml(&self) -> Result<String> {
        self.validate()?;
        let raw = Document {
            format_version: self.format_version,
            name: self.name.clone(),
            author: self.author.clone(),
            notes: self.notes.clone(),
            assets: self
                .assets
                .iter()
                .map(|(label, a)| (label.clone(), RawAsset::from(a)))
                .collect(),
            stages: self.stages.iter().map(RawStage::from).collect(),
        };
        toml::to_string_pretty(&raw).map_err(|e| Error::Write(e.to_string()))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format_version: u32,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    assets: BTreeMap<String, RawAsset>,
    #[serde(default, rename = "stage", skip_serializing_if = "Vec::is_empty")]
    stages: Vec<RawStage>,
}

/// An asset as written: `kind` and `hash` are text here, so that a document
/// that gets them wrong is told which one and why, rather than being handed
/// whatever the deserializer made of the failure.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAsset {
    kind: String,
    hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample_rate: Option<f64>,
}

impl RawAsset {
    fn into_asset(self) -> Result<Asset> {
        Ok(Asset {
            kind: AssetKind::parse(&self.kind)?,
            hash: Hash::parse(&self.hash)?,
            file: self.file,
            sample_rate: self.sample_rate,
        })
    }
}

impl From<&Asset> for RawAsset {
    fn from(a: &Asset) -> Self {
        Self {
            kind: a.kind.as_str().to_string(),
            hash: a.hash.as_str().to_string(),
            file: a.file.clone(),
            sample_rate: a.sample_rate,
        }
    }
}

/// A stage as written: one table that is either a module or an array of
/// lanes. Which one it is comes from what it carries, because TOML cannot
/// tag it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStage {
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    type_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    asset: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    bypass: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    params: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    ui: BTreeMap<String, String>,
    #[serde(default, rename = "lane", skip_serializing_if = "Vec::is_empty")]
    lanes: Vec<RawLane>,
}

impl RawStage {
    fn into_stage(self, index: usize) -> Result<Stage> {
        let looks_like_a_module = self.type_id.is_some()
            || self.name.is_some()
            || self.asset.is_some()
            || !self.params.is_empty()
            || !self.ui.is_empty();
        match (looks_like_a_module, self.lanes.is_empty()) {
            (true, false) => Err(Error::Shape(format!(
                "stage {index}: a stage is a module (`type` and `name`) or lanes (`lane`), not both"
            ))),
            (false, true) => Err(Error::Shape(format!(
                "stage {index}: a stage is a module (`type` and `name`) or lanes (`lane`); this one is neither"
            ))),
            (false, false) => Ok(Stage::Lanes(
                self.lanes.into_iter().map(RawLane::into_lane).collect(),
            )),
            (true, true) => Ok(Stage::Module(Module {
                type_id: self.type_id.ok_or_else(|| {
                    Error::Shape(format!("stage {index}: a module needs a `type`"))
                })?,
                name: self.name.ok_or_else(|| {
                    Error::Shape(format!("stage {index}: a module needs a `name`"))
                })?,
                asset: self.asset,
                bypass: self.bypass,
                params: self.params,
                ui: self.ui,
            })),
        }
    }
}

impl From<&Stage> for RawStage {
    fn from(s: &Stage) -> Self {
        match s {
            Stage::Module(m) => Self {
                type_id: Some(m.type_id.clone()),
                name: Some(m.name.clone()),
                asset: m.asset.clone(),
                bypass: m.bypass,
                params: m.params.clone(),
                ui: m.ui.clone(),
                lanes: Vec::new(),
            },
            Stage::Lanes(lanes) => Self {
                type_id: None,
                name: None,
                asset: None,
                bypass: false,
                params: BTreeMap::new(),
                ui: BTreeMap::new(),
                lanes: lanes.iter().map(RawLane::from).collect(),
            },
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLane {
    name: String,
    #[serde(default)]
    level_db: f64,
    #[serde(default)]
    pan: f64,
    #[serde(default, skip_serializing_if = "is_false")]
    bypass: bool,
    #[serde(default, rename = "module", skip_serializing_if = "Vec::is_empty")]
    modules: Vec<RawModule>,
}

impl RawLane {
    fn into_lane(self) -> Lane {
        Lane {
            name: self.name,
            level_db: self.level_db,
            pan: self.pan,
            bypass: self.bypass,
            modules: self
                .modules
                .into_iter()
                .map(RawModule::into_module)
                .collect(),
        }
    }
}

impl From<&Lane> for RawLane {
    fn from(l: &Lane) -> Self {
        Self {
            name: l.name.clone(),
            level_db: l.level_db,
            pan: l.pan,
            bypass: l.bypass,
            modules: l.modules.iter().map(RawModule::from).collect(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModule {
    #[serde(rename = "type")]
    type_id: String,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    asset: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    bypass: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    params: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    ui: BTreeMap<String, String>,
}

impl RawModule {
    fn into_module(self) -> Module {
        Module {
            type_id: self.type_id,
            name: self.name,
            asset: self.asset,
            bypass: self.bypass,
            params: self.params,
            ui: self.ui,
        }
    }
}

impl From<&Module> for RawModule {
    fn from(m: &Module) -> Self {
        Self {
            type_id: m.type_id.clone(),
            name: m.name.clone(),
            asset: m.asset.clone(),
            bypass: m.bypass,
            params: m.params.clone(),
            ui: m.ui.clone(),
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}
