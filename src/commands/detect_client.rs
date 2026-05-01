use std::time::Duration;

use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Detection {
    pub bbox: [f32; 4],
    pub label: String,
    pub confidence: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum DetectClientError {
    #[error("detection service is busy")]
    ServiceBusy,
    #[error("image rejected by detection service: {0}")]
    BadRequest(String),
    #[error("detection service error (HTTP {0})")]
    ServerError(u16),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn detect_low_priority(
    client: &Client,
    detect_url: &str,
    image_bytes: Vec<u8>,
    file_name: &str,
    mime: &str,
) -> Result<Vec<Detection>, DetectClientError> {
    let part = Part::bytes(image_bytes)
        .file_name(file_name.to_string())
        .mime_str(mime)?;
    let form = Form::new().part("image", part);

    let resp = client
        .post(detect_url)
        .header("X-Priority", "low")
        .multipart(form)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await?;

    let status = resp.status();
    if status.is_success() {
        return Ok(resp.json().await?);
    }

    let code = status.as_u16();
    match code {
        503 => Err(DetectClientError::ServiceBusy),
        400..=499 => {
            let body = resp.text().await.unwrap_or_default();
            Err(DetectClientError::BadRequest(body))
        }
        _ => Err(DetectClientError::ServerError(code)),
    }
}
