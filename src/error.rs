//! Error type for parsing and transliteration.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// Grammar rejected the map source.
    Parse {
        map: String,
        offset: usize,
        expected: Vec<String>,
    },
    /// The map name could not be resolved on the load path.
    MapNotFound(String),
    /// A dependency chain closed on itself.
    DependencyCycle(Vec<String>),
    /// The map uses a construct this runtime does not implement
    /// (legacy .imp DSL, rababa/secryst funcalls, reverse runs).
    Unsupported(String),
    /// A semantic error the Ruby runtime also rejects
    /// (MapLogicError / SystemConversionError parity).
    MapLogic(String),
    /// An IMF v1 artifact / registry / inference error
    /// (interscript-models SPEC.md violations included).
    Model(String),
    /// Filesystem / IO failure.
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse { map, offset, expected } => write!(
                f,
                "parse failed in {map} at offset {offset} (expected one of: {})",
                expected.join(", ")
            ),
            Error::MapNotFound(name) => write!(f, "couldn't locate map {name:?}"),
            Error::DependencyCycle(chain) => {
                write!(f, "dependency cycle: {}", chain.join(" -> "))
            }
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::MapLogic(what) => write!(f, "map logic error: {what}"),
            Error::Model(what) => write!(f, "model error: {what}"),
            Error::Io(what) => write!(f, "io error: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
