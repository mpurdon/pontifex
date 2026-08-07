use serde::Serialize;

/// Every Tauri command returns this. It serializes to a plain object so the
/// frontend can branch on `kind` (e.g. show a "Sign in" button for `Auth`)
/// instead of pattern-matching on error strings.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    /// Credentials are missing, expired, or the SSO token needs a refresh.
    #[error("{0}")]
    Auth(String),

    /// The caller is authenticated but not permitted to perform the action.
    #[error("{0}")]
    Forbidden(String),

    /// The requested registry / schema / log group does not exist.
    #[error("{0}")]
    NotFound(String),

    /// The user's input failed validation before we ever called AWS.
    #[error("{0}")]
    Invalid(String),

    /// An AWS API call failed for some other reason.
    #[error("{0}")]
    Aws(String),

    /// Local IO, config parsing, or anything else unexpected.
    #[error("{0}")]
    Internal(String),
}

impl Error {
    fn kind(&self) -> &'static str {
        match self {
            Error::Auth(_) => "auth",
            Error::Forbidden(_) => "forbidden",
            Error::NotFound(_) => "notFound",
            Error::Invalid(_) => "invalid",
            Error::Aws(_) => "aws",
            Error::Internal(_) => "internal",
        }
    }

    pub fn internal(e: impl std::fmt::Display) -> Self {
        Error::Internal(e.to_string())
    }

}

impl Serialize for Error {
    // Note the fully-qualified `std::result::Result`: this module's `Result`
    // alias fixes the error type, which is not what a Serialize impl returns.
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("Error", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Internal(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Invalid(format!("invalid JSON: {e}"))
    }
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::Internal(format!("{e:#}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Classify an AWS SDK error by inspecting its service error code.
///
/// The SDK's typed error enums differ per service, so every service module
/// funnels its errors through here with the code it managed to extract. This
/// keeps "your SSO token expired" from surfacing as an opaque dispatch failure.
pub fn classify(code: Option<&str>, message: String) -> Error {
    match code {
        Some("NotFoundException") | Some("ResourceNotFoundException") => Error::NotFound(message),
        Some("ForbiddenException") | Some("AccessDeniedException")
        | Some("AccessDeniedError") | Some("UnauthorizedException") => Error::Forbidden(message),
        Some("ExpiredTokenException") | Some("ExpiredToken")
        | Some("UnrecognizedClientException") | Some("InvalidClientTokenId") => {
            Error::Auth(message)
        }
        Some("BadRequestException") | Some("ValidationException")
        | Some("ConflictException") => Error::Invalid(message),
        _ => {
            // Credential resolution failures surface as dispatch errors with no
            // service code, so fall back to sniffing the text.
            //
            // Getting this right matters: an `Auth` error is what makes the UI
            // offer a "Sign in" button instead of a dead-end "no access" badge.
            // The most common first-run failure is a profile whose SSO token
            // cache file does not exist yet, which reads as a plain
            // file-not-found buried inside a credentials-loading error.
            let lowered = message.to_lowercase();
            let credential_failure = lowered.contains("loading credentials")
                || lowered.contains("no credentials")
                || lowered.contains("credentials were not loaded")
                || lowered.contains("credentials provider")
                || lowered.contains("profilefileloaderror");
            let sso_problem = lowered.contains("sso")
                || lowered.contains("token has expired")
                || lowered.contains("reauthenticate");

            if credential_failure || sso_problem || lowered.contains("expired") {
                Error::Auth(message)
            } else {
                Error::Aws(message)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_of(code: Option<&str>, message: &str) -> &'static str {
        classify(code, message.to_string()).kind()
    }

    #[test]
    fn classifies_service_error_codes() {
        assert_eq!(kind_of(Some("NotFoundException"), "gone"), "notFound");
        assert_eq!(kind_of(Some("ForbiddenException"), "nope"), "forbidden");
        assert_eq!(kind_of(Some("ExpiredTokenException"), "stale"), "auth");
        assert_eq!(kind_of(Some("BadRequestException"), "bad"), "invalid");
    }

    #[test]
    fn treats_a_missing_sso_token_cache_as_an_auth_problem() {
        // The real message the SDK produces for a profile that has never been
        // signed into. It must offer a sign-in, not a generic failure.
        let message = "dispatch failure: other: an error occurred while loading \
                       credentials: failed to read \
                       `/Users/mp/.aws/sso/cache/f8e4a42b.json`: \
                       No such file or directory (os error 2)";
        assert_eq!(kind_of(None, message), "auth");
    }

    #[test]
    fn treats_other_credential_failures_as_auth_problems() {
        for message in [
            "no credentials in the property bag",
            "the credentials provider was not enabled",
            "Error retrieving credentials from the instance IMDS role: token has expired",
            "SSO session is expired, please reauthenticate",
        ] {
            assert_eq!(kind_of(None, message), "auth", "for: {message}");
        }
    }

    #[test]
    fn leaves_genuine_api_failures_alone() {
        for message in [
            "service error: InternalServerError",
            "connection reset by peer",
            "the request was throttled",
        ] {
            assert_eq!(kind_of(None, message), "aws", "for: {message}");
        }
    }
}
