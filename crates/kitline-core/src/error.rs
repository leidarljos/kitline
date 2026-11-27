use std::fmt;

/// Failures from identity, revision, and kit operations.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Git(String),
    Id(String),
    Trailer(String),
    /// The jj `change-id` header and the `Kitline-Change` trailer both
    /// parse, and they are not the same change.
    Disagree {
        header: String,
        trailer: String,
    },
    Kit(String),
    Deed(String),
    /// A merge commit, named by its object id.
    Merge(String),
    /// A commit with no tree change, named by its object id.
    Empty(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(s) => write!(f, "git: {s}"),
            Self::Id(s) => write!(f, "id: {s}"),
            Self::Trailer(s) => write!(f, "trailer: {s}"),
            Self::Disagree { header, trailer } => {
                write!(f, "refused: header {header} and trailer {trailer} disagree")
            }
            Self::Kit(s) => write!(f, "kit: {s}"),
            Self::Deed(s) => write!(f, "deed: {s}"),
            Self::Merge(commit) => write!(f, "merge commit {commit}"),
            Self::Empty(commit) => write!(f, "empty commit {commit}"),
        }
    }
}

impl std::error::Error for Error {}
