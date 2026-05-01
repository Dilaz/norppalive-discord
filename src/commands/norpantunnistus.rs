use std::io::Cursor;
use std::sync::Arc;

use image::ImageReader;
use serenity::all::{
    CommandDataOptionValue, CommandInteraction, CommandType, Context, CreateAttachment,
    CreateInteractionResponse, CreateInteractionResponseFollowup, CreateInteractionResponseMessage,
};

use super::detect_client::{self, DetectClientError};
use super::draw;
use super::rate_limit::{RateLimitError, RateLimiter};

const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_IMAGE_DIM: u32 = 4096;
const MAX_DETECT_WIDTH: u32 = 1280;
const MAX_DETECT_HEIGHT: u32 = 720;
const SUPPORTED_MIMES: &[&str] = &["image/jpeg", "image/png", "image/webp", "image/bmp"];

pub const SLASH_COMMAND_NAME: &str = "norpantunnistus";
pub const CONTEXT_COMMAND_NAME: &str = "Tunnista norppa";

pub struct CommandDeps {
    pub rate_limiter: Arc<RateLimiter>,
    pub semaphore: Arc<tokio::sync::Semaphore>,
    pub http_client: reqwest::Client,
    pub detect_url: String,
}

pub async fn handle(ctx: Context, cmd: CommandInteraction, deps: Arc<CommandDeps>) {
    // 1. Rate limit — fail fast before defer so the user gets a quick reply.
    if let Err(e) = deps.rate_limiter.check(cmd.user.id) {
        respond_ephemeral(
            &ctx,
            &cmd,
            &format_rate_limit(&e, deps.rate_limiter.daily_max()),
        )
        .await;
        return;
    }

    // 2. Resolve which image to process.
    let source = match resolve_image(&cmd) {
        Ok(s) => s,
        Err(e) => {
            respond_ephemeral(&ctx, &cmd, e.user_message()).await;
            return;
        }
    };

    // 3. Defer — buys 15 minutes before Discord retracts the interaction.
    if let Err(e) = cmd.defer(&ctx.http).await {
        tracing::warn!("failed to defer interaction: {e}");
        return;
    }

    // 4. Bot-side concurrency cap on /detect calls.
    let _permit = match deps.semaphore.clone().acquire_owned().await {
        Ok(p) => p,
        Err(_) => {
            followup_ephemeral(&ctx, &cmd, "Bot suljetaan, yritä uudelleen myöhemmin.").await;
            return;
        }
    };

    // 5. Download the image bytes.
    let bytes = match fetch_bytes(&deps.http_client, &source.url, MAX_IMAGE_BYTES).await {
        Ok(b) => b,
        Err(FetchError::TooLarge) => {
            followup_ephemeral(&ctx, &cmd, "Kuva on liian suuri (max 20 MB).").await;
            return;
        }
        Err(e) => {
            tracing::warn!("image fetch failed: {e:?}");
            followup_ephemeral(&ctx, &cmd, "Kuvan lataus epäonnistui.").await;
            return;
        }
    };

    // 6. Dimension peek before forwarding — early-reject decompression bombs.
    match peek_dimensions(&bytes) {
        Ok(_) => {}
        Err(PeekError::TooLarge) => {
            followup_ephemeral(&ctx, &cmd, "Kuva on liian iso (max 4096×4096).").await;
            return;
        }
        Err(PeekError::Unsupported) => {
            followup_ephemeral(&ctx, &cmd, "Tiedosto ei ole tuettu kuvamuoto.").await;
            return;
        }
        Err(PeekError::Decode) => {
            followup_ephemeral(&ctx, &cmd, "Kuvan lukeminen epäonnistui.").await;
            return;
        }
    }

    // 7. Downscale to <=1280x720 — keeps detection fast and the annotated
    //    followup small enough not to blow Discord's upload limit.
    let (bytes, detect_mime) = match downscale_for_detect(bytes, &source.mime) {
        Ok(b) => b,
        Err(_) => {
            followup_ephemeral(&ctx, &cmd, "Kuvan käsittely epäonnistui.").await;
            return;
        }
    };

    // 8. Call detection API with low priority.
    let detections = match detect_client::detect_low_priority(
        &deps.http_client,
        &deps.detect_url,
        bytes.clone(),
        "image.bin",
        detect_mime.as_str(),
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            followup_ephemeral(&ctx, &cmd, format_detect_error(&e)).await;
            return;
        }
    };

    // 9. Successful API call — burn one quota slot.
    deps.rate_limiter.record(cmd.user.id);

    // 10. Reply.
    if detections.is_empty() {
        followup_ephemeral(&ctx, &cmd, "Ei norppia havaittu.").await;
        return;
    }

    let annotated = match draw::draw_bboxes(&bytes, &detections) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("draw failed: {e}");
            followup_ephemeral(&ctx, &cmd, "Kuvan piirto epäonnistui.").await;
            return;
        }
    };

    let attachment = CreateAttachment::bytes(annotated, "norpantunnistus.jpg");
    let followup = CreateInteractionResponseFollowup::new().add_file(attachment);
    if let Err(e) = cmd.create_followup(&ctx.http, followup).await {
        tracing::error!("failed to send followup with annotated image: {e}");
    }
}

// -------- image source resolution --------

#[derive(Debug)]
struct ImageSource {
    url: String,
    mime: String,
}

#[derive(Debug, thiserror::Error)]
enum ResolveError {
    #[error("no image attached")]
    NoImageAttached,
    #[error("no image on target message")]
    NoImageOnMessage,
    #[error("unsupported image format")]
    UnsupportedFormat,
    #[error("image too large")]
    TooLarge,
}

impl ResolveError {
    fn user_message(&self) -> &'static str {
        match self {
            ResolveError::NoImageAttached => "Liitä kuva komentoon.",
            ResolveError::NoImageOnMessage => "Tässä viestissä ei ole kuvaa.",
            ResolveError::UnsupportedFormat => "Tiedosto ei ole tuettu kuvamuoto.",
            ResolveError::TooLarge => "Kuva on liian suuri (max 20 MB).",
        }
    }
}

fn resolve_image(cmd: &CommandInteraction) -> Result<ImageSource, ResolveError> {
    match cmd.data.kind {
        CommandType::ChatInput => resolve_slash(cmd),
        CommandType::Message => resolve_context(cmd),
        _ => Err(ResolveError::NoImageAttached),
    }
}

fn resolve_slash(cmd: &CommandInteraction) -> Result<ImageSource, ResolveError> {
    let attachment_id = cmd
        .data
        .options
        .iter()
        .find(|opt| opt.name == "image")
        .and_then(|opt| match &opt.value {
            CommandDataOptionValue::Attachment(id) => Some(*id),
            _ => None,
        })
        .ok_or(ResolveError::NoImageAttached)?;
    let attachment = cmd
        .data
        .resolved
        .attachments
        .get(&attachment_id)
        .ok_or(ResolveError::NoImageAttached)?;

    if (attachment.size as u64) > MAX_IMAGE_BYTES {
        return Err(ResolveError::TooLarge);
    }
    let mime = attachment
        .content_type
        .as_deref()
        .ok_or(ResolveError::UnsupportedFormat)?;
    if !is_supported_mime(mime) {
        return Err(ResolveError::UnsupportedFormat);
    }
    Ok(ImageSource {
        url: attachment.url.clone(),
        mime: normalize_mime(mime),
    })
}

fn resolve_context(cmd: &CommandInteraction) -> Result<ImageSource, ResolveError> {
    let target_msg = cmd
        .data
        .resolved
        .messages
        .values()
        .next()
        .ok_or(ResolveError::NoImageOnMessage)?;

    for att in &target_msg.attachments {
        let mime = att.content_type.as_deref().unwrap_or("");
        if is_supported_mime(mime) {
            if (att.size as u64) > MAX_IMAGE_BYTES {
                return Err(ResolveError::TooLarge);
            }
            return Ok(ImageSource {
                url: att.url.clone(),
                mime: normalize_mime(mime),
            });
        }
    }

    for emb in &target_msg.embeds {
        if let Some(image) = &emb.image {
            return Ok(ImageSource {
                url: image.url.clone(),
                mime: "image/jpeg".to_string(),
            });
        }
        if let Some(thumb) = &emb.thumbnail {
            return Ok(ImageSource {
                url: thumb.url.clone(),
                mime: "image/jpeg".to_string(),
            });
        }
    }
    Err(ResolveError::NoImageOnMessage)
}

fn is_supported_mime(mime: &str) -> bool {
    let m = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    SUPPORTED_MIMES.iter().any(|s| *s == m)
}

fn normalize_mime(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase()
}

// -------- helpers --------

#[derive(Debug, thiserror::Error)]
enum FetchError {
    #[error("image too large")]
    TooLarge,
    #[error("upstream returned HTTP {0}")]
    HttpStatus(u16),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
}

async fn fetch_bytes(
    client: &reqwest::Client,
    url: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, FetchError> {
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(FetchError::HttpStatus(resp.status().as_u16()));
    }
    if let Some(len) = resp.content_length() {
        if len > max_bytes {
            return Err(FetchError::TooLarge);
        }
    }
    let bytes = resp.bytes().await?;
    if (bytes.len() as u64) > max_bytes {
        return Err(FetchError::TooLarge);
    }
    Ok(bytes.to_vec())
}

#[derive(Debug)]
enum PeekError {
    TooLarge,
    Unsupported,
    Decode,
}

fn peek_dimensions(bytes: &[u8]) -> Result<(u32, u32), PeekError> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| PeekError::Unsupported)?;
    let (w, h) = reader.into_dimensions().map_err(|_| PeekError::Decode)?;
    if w > MAX_IMAGE_DIM || h > MAX_IMAGE_DIM || (w as u64 * h as u64) > 16_000_000 {
        return Err(PeekError::TooLarge);
    }
    Ok((w, h))
}

fn downscale_for_detect(bytes: Vec<u8>, mime: &str) -> Result<(Vec<u8>, String), PeekError> {
    let reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| PeekError::Unsupported)?;
    let img = reader.decode().map_err(|_| PeekError::Decode)?;
    if img.width() <= MAX_DETECT_WIDTH && img.height() <= MAX_DETECT_HEIGHT {
        return Ok((bytes, mime.to_string()));
    }
    let scaled = img.resize(
        MAX_DETECT_WIDTH,
        MAX_DETECT_HEIGHT,
        image::imageops::FilterType::Lanczos3,
    );
    tracing::debug!(
        "downscaled image {}x{} -> {}x{}",
        img.width(),
        img.height(),
        scaled.width(),
        scaled.height()
    );
    let mut out = Vec::with_capacity(bytes.len() / 2);
    scaled
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg)
        .map_err(|_| PeekError::Decode)?;
    Ok((out, "image/jpeg".to_string()))
}

fn format_rate_limit(e: &RateLimitError, daily_max: u32) -> String {
    match e {
        RateLimitError::Cooldown { retry_in } => {
            let secs = retry_in.as_secs().max(1);
            format!("Odota {secs} sekuntia ennen seuraavaa tunnistusta.")
        }
        RateLimitError::DailyQuota => {
            format!("Päivittäinen raja ({daily_max}) täynnä. Kokeile huomenna.")
        }
    }
}

fn format_detect_error(e: &DetectClientError) -> &'static str {
    match e {
        DetectClientError::ServiceBusy => {
            "Tunnistuspalvelu on kiireinen, yritä hetken päästä uudelleen."
        }
        DetectClientError::BadRequest(_) => "Kuvan käsittely epäonnistui (kuva ei kelpaa).",
        DetectClientError::ServerError(_) | DetectClientError::Network(_) => {
            "Tunnistuspalvelu ei vastaa, yritä hetken päästä uudelleen."
        }
    }
}

async fn respond_ephemeral(ctx: &Context, cmd: &CommandInteraction, content: &str) {
    let response = CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .content(content)
            .ephemeral(true),
    );
    if let Err(e) = cmd.create_response(&ctx.http, response).await {
        tracing::error!("failed to send ephemeral response: {e}");
    }
}

async fn followup_ephemeral(ctx: &Context, cmd: &CommandInteraction, content: impl Into<String>) {
    let followup = CreateInteractionResponseFollowup::new()
        .content(content)
        .ephemeral(true);
    if let Err(e) = cmd.create_followup(&ctx.http, followup).await {
        tracing::error!("failed to send ephemeral followup: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_mime_recognises_common_images() {
        assert!(is_supported_mime("image/jpeg"));
        assert!(is_supported_mime("IMAGE/JPEG"));
        assert!(is_supported_mime("image/png"));
        assert!(is_supported_mime("image/webp"));
        assert!(is_supported_mime("image/bmp"));
        assert!(is_supported_mime("image/jpeg; charset=binary"));
    }

    #[test]
    fn unsupported_mime_is_rejected() {
        assert!(!is_supported_mime("image/gif"));
        assert!(!is_supported_mime("application/octet-stream"));
        assert!(!is_supported_mime(""));
        assert!(!is_supported_mime("text/plain"));
    }

    #[test]
    fn peek_accepts_normal_image() {
        let img = image::DynamicImage::new_rgb8(640, 480);
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
            .unwrap();
        let (w, h) = peek_dimensions(&bytes).unwrap();
        assert_eq!((w, h), (640, 480));
    }

    #[test]
    fn peek_rejects_huge_dimensions() {
        let img = image::DynamicImage::new_rgb8(MAX_IMAGE_DIM + 1, 100);
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        assert!(matches!(peek_dimensions(&bytes), Err(PeekError::TooLarge)));
    }

    #[test]
    fn peek_rejects_garbage() {
        assert!(matches!(
            peek_dimensions(b"this is not an image"),
            Err(PeekError::Unsupported | PeekError::Decode)
        ));
    }

    #[test]
    fn rate_limit_message_includes_seconds() {
        let e = RateLimitError::Cooldown {
            retry_in: std::time::Duration::from_secs(7),
        };
        let msg = format_rate_limit(&e, 20);
        assert!(msg.contains('7'));
        assert!(msg.to_lowercase().contains("odota"));
    }

    #[test]
    fn rate_limit_message_for_daily_includes_max() {
        let msg = format_rate_limit(&RateLimitError::DailyQuota, 20);
        assert!(msg.contains("20"));
        assert!(msg.contains("Päivittäinen"));
    }

    #[test]
    fn downscale_passes_through_already_small_image() {
        let img = image::DynamicImage::new_rgb8(640, 480);
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let original = bytes.clone();
        let (out, mime) = downscale_for_detect(bytes, "image/png").unwrap();
        assert_eq!(out, original);
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn downscale_passes_through_image_at_exact_bounds() {
        let img = image::DynamicImage::new_rgb8(MAX_DETECT_WIDTH, MAX_DETECT_HEIGHT);
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
            .unwrap();
        let original = bytes.clone();
        let (out, _mime) = downscale_for_detect(bytes, "image/jpeg").unwrap();
        assert_eq!(out, original);
    }

    #[test]
    fn downscale_resizes_oversized_landscape() {
        let img = image::DynamicImage::new_rgb8(2560, 1440);
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
            .unwrap();
        let (out, mime) = downscale_for_detect(bytes, "image/jpeg").unwrap();
        let (w, h) = ImageReader::new(Cursor::new(&out))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap();
        assert!(w <= MAX_DETECT_WIDTH && h <= MAX_DETECT_HEIGHT);
        assert_eq!((w, h), (1280, 720));
        assert_eq!(mime, "image/jpeg");
    }

    #[test]
    fn downscale_resizes_oversized_portrait() {
        let img = image::DynamicImage::new_rgb8(1080, 1920);
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
            .unwrap();
        let (out, _mime) = downscale_for_detect(bytes, "image/jpeg").unwrap();
        let (w, h) = ImageReader::new(Cursor::new(&out))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap();
        assert!(w <= MAX_DETECT_WIDTH && h <= MAX_DETECT_HEIGHT);
        assert_eq!(h, 720);
    }

    #[test]
    fn downscale_re_encodes_png_to_jpeg() {
        let img = image::DynamicImage::new_rgb8(2000, 2000);
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let (_out, mime) = downscale_for_detect(bytes, "image/png").unwrap();
        assert_eq!(mime, "image/jpeg");
    }
}
