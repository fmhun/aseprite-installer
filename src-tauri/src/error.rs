use serde::Serialize;
use std::fmt::{Display, Formatter};

pub type AppResult<T> = Result<T, InstallerError>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallerError {
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
}

impl InstallerError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(
        code: impl Into<String>,
        message: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: Some(detail.into()),
        }
    }
}

impl Display for InstallerError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for InstallerError {}

impl From<std::io::Error> for InstallerError {
    fn from(error: std::io::Error) -> Self {
        Self::with_detail("io", "A file operation failed.", error.to_string())
    }
}

impl From<reqwest::Error> for InstallerError {
    fn from(error: reqwest::Error) -> Self {
        Self::with_detail(
            "network",
            "The official Aseprite release data could not be downloaded.",
            error.to_string(),
        )
    }
}

impl From<serde_json::Error> for InstallerError {
    fn from(error: serde_json::Error) -> Self {
        Self::with_detail(
            "invalidData",
            "Stored installer data is invalid.",
            error.to_string(),
        )
    }
}
