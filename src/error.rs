//! Errors from building and preparing a rack. `process` has none.

use thiserror::Error;
use valverig_format::AssetKind;

/// Anything that can go wrong before the audio thread starts.
#[derive(Debug, Error)]
pub enum Error {
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
    /// Two modules or lanes were given the same name.
    #[error("two modules or lanes named {0:?}")]
    DuplicateName(String),
    /// A lanes stage was given no lanes.
    #[error("a lanes stage needs at least one lane")]
    NoLanes,
    /// No module or lane has that name.
    #[error("no module or lane named {0:?}")]
    UnknownName(String),
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
    /// The preset itself is wrong; the format says how.
    #[error(transparent)]
    Format(#[from] valverig_format::Error),
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
