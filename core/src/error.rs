use std::fmt;

/// One error type for the whole core. `Clone` so it can travel inside UI messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The request never got an answer, or got a non-success status.
    Network(String),
    /// YouTube answered with a shape this client doesn't understand.
    Parse(String),
    /// Replay mode was asked for something that was never recorded.
    Fixture(String),
    /// libmpv refused something.
    Player(String),
    /// The job was dropped before it finished.
    Cancelled,
    Io(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Network(m) => write!(f, "Network error: {m}"),
            Error::Parse(m) => write!(f, "Unexpected response: {m}"),
            Error::Fixture(m) => write!(f, "Missing fixture: {m}"),
            Error::Player(m) => write!(f, "Playback error: {m}"),
            Error::Cancelled => write!(f, "Cancelled"),
            Error::Io(m) => write!(f, "I/O error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Network(e.without_url().to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Parse(e.to_string())
    }
}
