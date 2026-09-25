//! Everything that can go wrong before the audio thread starts: a `.rig`
//! document that is malformed, a rack that cannot be built from one, and a
//! rack that cannot be prepared. `process` has none.

use crate::preset::AssetKind;
use thiserror::Error;

/// Anything that can go wrong before the audio thread starts.
///
/// Every variant names the part at fault, so a host can put the message in
/// front of a person without adding context of its own.
#[derive(Debug, Error)]
pub enum Error {
    /// Not TOML, or not the tables and keys the `.rig` format expects. The
    /// message carries the parser's own report, which names the line and
    /// column.
    #[error("not a valid .rig document: {0}")]
    Toml(String),

    /// The document was written to a different version of the format.
    #[error("format_version {found}: this reader reads and writes version {supported}")]
    Version {
        /// The version the document declares.
        found: u32,
        /// The only version this crate handles.
        supported: u32,
    },

    /// A stage is neither a module nor a set of lanes, or is both.
    #[error("{0}")]
    Shape(String),

    /// Two modules or lanes claim the same name, in a preset or in a rack
    /// being built. Names are the first half of every address, so they have
    /// to be unique across the whole rack.
    #[error("two modules or lanes are named {0:?}")]
    DuplicateName(String),

    /// No module or lane has that name: not in the preset a caller passed to
    /// [`Preset::set_value`] or [`Preset::set_bypass`], and not on the rack
    /// the controls belong to.
    ///
    /// [`Preset::set_value`]: crate::preset::Preset::set_value
    /// [`Preset::set_bypass`]: crate::preset::Preset::set_bypass
    #[error("no module or lane is named {0:?}")]
    UnknownName(String),

    /// A module names an asset label the preset does not carry.
    #[error("module {module:?} names asset {asset:?}, which the preset does not carry")]
    UnknownAsset {
        /// The module that asked.
        module: String,
        /// The label it asked for.
        asset: String,
    },

    /// A module or lane name that cannot be half of an address.
    #[error("{0:?} is not a name: names are 1 to {max} bytes and contain no `/` and no control characters", max = crate::preset::text::NAME_MAX)]
    BadName(String),

    /// A module type that is not `namespace.name`.
    #[error("{0:?} is not a module type: expected `namespace.name`, each part {rule}", rule = crate::preset::text::IDENTIFIER_RULE)]
    BadType(String),

    /// A parameter id that cannot be the second half of an address.
    #[error("module {module:?} has a parameter id {id:?}, which is not an identifier: {rule}", rule = crate::preset::text::IDENTIFIER_RULE)]
    BadParamId {
        /// The module carrying it.
        module: String,
        /// The id at fault.
        id: String,
    },

    /// A presentation hint key that is not an identifier.
    #[error("module {module:?} has a presentation hint key {key:?}, which is not an identifier: {rule}", rule = crate::preset::text::IDENTIFIER_RULE)]
    BadUiKey {
        /// The module carrying it.
        module: String,
        /// The key at fault.
        key: String,
    },

    /// An asset label that is not an identifier.
    #[error("{0:?} is not an asset label: {rule}", rule = crate::preset::text::IDENTIFIER_RULE)]
    BadAssetLabel(String),

    /// Not a content address.
    #[error("{0:?} is not a content address: expected `blake3:` and 64 lowercase hex digits")]
    BadHash(String),

    /// Not a kind of asset the format knows.
    #[error("{0:?} is not a kind of asset: expected \"nam\" or \"ir\"")]
    BadAssetKind(String),

    /// An asset's `file` hint is not a plain file name.
    ///
    /// The hint helps a person and a resolver recognise an asset; it never
    /// says where the bytes live. A document that could name a path would be
    /// telling every host which file to open.
    #[error("asset {asset:?} has file = {file:?}, which is not a plain file name: {rule}", rule = crate::preset::text::FILE_NAME_RULE)]
    BadFile {
        /// The asset label.
        asset: String,
        /// The hint at fault.
        file: String,
    },

    /// An asset's stated sample rate is not one an audio file could have.
    #[error("asset {asset:?} states sample_rate = {value}, which is not a rate between {min} and {max} Hz", min = crate::MIN_SAMPLE_RATE, max = crate::MAX_SAMPLE_RATE)]
    BadSampleRate {
        /// The asset label.
        asset: String,
        /// The rate at fault.
        value: f64,
    },

    /// An asset states a sample rate on a kind that carries its own.
    #[error("asset {0:?} is a capture and states a sample_rate; a capture carries its own rate")]
    UnwantedSampleRate(String),

    /// A number that is not one: `nan` or an infinity, at the address given.
    #[error("{0} is not a finite number")]
    NotFinite(String),

    /// A preset declares more of something than a reader will build. Every
    /// count in a `.rig` reaches an allocation in whoever builds the rack,
    /// so each one is capped where the file is still just text.
    #[error("this preset has {found} {what}, and the most allowed is {max}")]
    TooMany {
        /// What there were too many of, plural: `stages`, `lanes in one
        /// stage`, `modules`, `assets`.
        what: &'static str,
        /// How many the preset declares.
        found: usize,
        /// The most a reader will accept.
        max: usize,
    },

    /// The document could not be written. The message carries the writer's
    /// own report.
    #[error("could not write the document: {0}")]
    Write(String),

    /// A rack was asked to run at a sample rate outside
    /// [`MIN_SAMPLE_RATE`](crate::MIN_SAMPLE_RATE) to
    /// [`MAX_SAMPLE_RATE`](crate::MAX_SAMPLE_RATE), or at one that is not a
    /// number.
    #[error("a rack runs at {min} to {max} Hz, not at {0} Hz", min = crate::MIN_SAMPLE_RATE, max = crate::MAX_SAMPLE_RATE)]
    UnsupportedSampleRate(f64),

    /// A module only works at one sample rate, and the rack is prepared at
    /// another.
    #[error(
        "{node} needs {needs} Hz but the rack is prepared at {got} Hz, and nothing here resamples"
    )]
    SampleRate {
        /// Which module, as `name (type)`.
        node: String,
        /// The rate the module needs.
        needs: f64,
        /// The rate the rack was prepared at.
        got: f64,
    },

    /// A module cannot take as many channels as the rack hands it where it
    /// sits: a capture after a lanes stage, say.
    #[error("{node} takes at most {max} channel(s), but the rack hands it {got} where it sits")]
    Channels {
        /// Which module, as `name (type)`.
        node: String,
        /// The most channels the module can take.
        max: usize,
        /// The width of the signal at the module.
        got: usize,
    },

    /// A lanes stage was given no lanes.
    #[error("a lanes stage needs at least one lane")]
    NoLanes,

    /// The module exists but has no parameter with that id.
    #[error("no parameter {0:?}")]
    UnknownParam(String),

    /// A parameter address is `name/parameter`.
    #[error("a parameter address is name/parameter, got {0:?}")]
    BadAddress(String),

    /// A preset names a module type this engine does not have.
    #[error("module {module:?} is a {type_id:?}, which this engine does not have")]
    UnknownType {
        /// The module's name in the preset.
        module: String,
        /// The type it asked for.
        type_id: String,
    },

    /// A module needs an asset and the preset names none for it.
    #[error("module {module:?} needs an asset, and the preset gives it none")]
    MissingAsset {
        /// The module's name in the preset.
        module: String,
    },

    /// A module's asset is there, but it is the other kind of asset.
    #[error("module {module:?} needs {needs}, but asset {asset:?} is {found}")]
    WrongAssetKind {
        /// The module's name in the preset.
        module: String,
        /// The asset's label in the preset.
        asset: String,
        /// What the module needs.
        needs: AssetKind,
        /// What the asset turned out to be.
        found: AssetKind,
    },

    /// The host's resolver could not load an asset.
    #[error("module {module:?}: asset {asset:?} would not load")]
    Resolve {
        /// The module's name in the preset.
        module: String,
        /// The asset's label in the preset.
        asset: String,
        /// Whatever the resolver said; the host owns this error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A lane worker thread could not be started.
    #[error("could not start a thread for lane {lane:?}")]
    Thread {
        /// The lane the thread was for.
        lane: String,
        /// Why the operating system refused.
        #[source]
        source: std::io::Error,
    },
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;
