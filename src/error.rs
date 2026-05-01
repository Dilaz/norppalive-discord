use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum NorppaliveError {
    #[error(transparent)]
    #[diagnostic(code(norppalive::io_error))]
    Io(#[from] std::io::Error),

    #[error("Environment variable error: {0}")]
    #[diagnostic(code(norppalive::env_var_error))]
    EnvVar(#[from] std::env::VarError),

    #[error("Serenity error: {0}")]
    #[diagnostic(code(norppalive::discord_error))]
    Discord(#[from] serenity::Error),

    #[error("Configuration error: {0}")]
    #[diagnostic(code(norppalive::config_error))]
    Config(String),

    #[error("Base64 decode error: {0}")]
    #[diagnostic(code(norppalive::base64_error))]
    Base64(#[from] base64::DecodeError),

    #[error("JSON error: {0}")]
    #[diagnostic(code(norppalive::json_error))]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: NorppaliveError = io_err.into();
        assert!(matches!(err, NorppaliveError::Io(_)));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn from_env_var_error() {
        let env_err = std::env::VarError::NotPresent;
        let err: NorppaliveError = env_err.into();
        assert!(matches!(err, NorppaliveError::EnvVar(_)));
    }

    #[test]
    fn from_base64_decode_error() {
        let b64_result = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            "not valid base64!!!",
        );
        let b64_err = b64_result.unwrap_err();
        let err: NorppaliveError = b64_err.into();
        assert!(matches!(err, NorppaliveError::Base64(_)));
    }

    #[test]
    fn from_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let err: NorppaliveError = json_err.into();
        assert!(matches!(err, NorppaliveError::Json(_)));
    }

    #[test]
    fn config_error_message() {
        let err = NorppaliveError::Config("bad config".into());
        assert_eq!(err.to_string(), "Configuration error: bad config");
    }

    #[test]
    fn env_var_error_display() {
        let err = NorppaliveError::EnvVar(std::env::VarError::NotPresent);
        assert!(err.to_string().contains("Environment variable error"));
    }

    #[test]
    fn diagnostic_codes() {
        use miette::Diagnostic;

        let io_err = NorppaliveError::Io(std::io::Error::other("x"));
        assert_eq!(io_err.code().unwrap().to_string(), "norppalive::io_error");

        let config_err = NorppaliveError::Config("x".into());
        assert_eq!(
            config_err.code().unwrap().to_string(),
            "norppalive::config_error"
        );

        let b64_err = NorppaliveError::Base64(
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, "!!!").unwrap_err(),
        );
        assert_eq!(
            b64_err.code().unwrap().to_string(),
            "norppalive::base64_error"
        );

        let json_err = NorppaliveError::Json(serde_json::from_str::<()>("x").unwrap_err());
        assert_eq!(
            json_err.code().unwrap().to_string(),
            "norppalive::json_error"
        );
    }
}
