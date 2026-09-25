use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(String, std::io::Error),
    Zip(String),
    Dex { input: String, error: eightr_dex::DexError },
    NoDexFiles(String),
    DuplicateClass { descriptor: String, inputs: [String; 2] },
    /// A pass tried to record an application of a rule that isn't in the registry, or for an
    /// attribute the rule doesn't declare. Always a bug in 8R.
    UnregisteredRule { rule: String, detail: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(path, e) => write!(f, "{path}: {e}"),
            Error::Zip(e) => write!(f, "zip: {e}"),
            Error::Dex { input, error } => write!(f, "{input}: {error}"),
            Error::NoDexFiles(path) => write!(f, "{path}: no classes*.dex found"),
            Error::DuplicateClass { descriptor, inputs } => {
                write!(f, "class {descriptor} defined in both {} and {}", inputs[0], inputs[1])
            }
            Error::UnregisteredRule { rule, detail } => write!(f, "internal error: rule {rule}: {detail}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
