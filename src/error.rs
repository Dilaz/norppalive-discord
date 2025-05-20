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
