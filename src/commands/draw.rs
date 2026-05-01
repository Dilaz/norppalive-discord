use std::io::Cursor;

use ab_glyph::{FontRef, PxScale};
use image::{DynamicImage, ImageFormat, ImageReader, Rgba};
use imageproc::drawing::{draw_filled_rect_mut, draw_hollow_rect_mut, draw_text_mut, text_size};
use imageproc::rect::Rect;

use super::detect_client::Detection;

const MAX_IMAGE_DIM: u32 = 4096;
const MAX_DECODE_ALLOC_BYTES: u64 = 128 * 1024 * 1024;

const LINE_THICKNESS: u32 = 4;
const BOX_COLOR: Rgba<u8> = Rgba([255, 32, 32, 255]);
const TEXT_COLOR: Rgba<u8> = Rgba([255, 255, 255, 255]);
const TEXT_BG_COLOR: Rgba<u8> = Rgba([255, 32, 32, 255]);
const TEXT_PX: f32 = 25.0;
const TEXT_PADDING_X: u32 = 10;
const TEXT_PADDING_Y: u32 = 5;

const FONT_BYTES: &[u8] = include_bytes!("../DejaVuSans.ttf");

#[derive(Debug, thiserror::Error)]
pub enum DrawError {
    #[error("image too large (max {max_dim}x{max_dim})")]
    ImageTooLarge { max_dim: u32 },
    #[error("image format not supported")]
    UnsupportedFormat,
    #[error("image decode failed: {0}")]
    Decode(#[from] image::ImageError),
    #[error("font load failed")]
    Font,
    #[error("encode failed: {0}")]
    Encode(image::ImageError),
}

pub fn draw_bboxes(image_bytes: &[u8], detections: &[Detection]) -> Result<Vec<u8>, DrawError> {
    let mut img = decode_with_limits(image_bytes)?;
    let font = FontRef::try_from_slice(FONT_BYTES).map_err(|_| DrawError::Font)?;

    for det in detections {
        draw_one(&mut img, &font, det);
    }

    let mut out = Vec::with_capacity(image_bytes.len());
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg)
        .map_err(DrawError::Encode)?;
    Ok(out)
}

fn decode_with_limits(bytes: &[u8]) -> Result<DynamicImage, DrawError> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| DrawError::UnsupportedFormat)?;
    let (w, h) = reader.into_dimensions()?;
    if w > MAX_IMAGE_DIM || h > MAX_IMAGE_DIM {
        return Err(DrawError::ImageTooLarge {
            max_dim: MAX_IMAGE_DIM,
        });
    }

    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| DrawError::UnsupportedFormat)?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    limits.max_image_width = Some(MAX_IMAGE_DIM);
    limits.max_image_height = Some(MAX_IMAGE_DIM);
    reader.limits(limits);
    Ok(reader.decode()?)
}

fn draw_one(img: &mut DynamicImage, font: &FontRef<'_>, det: &Detection) {
    let img_w = img.width();
    let img_h = img.height();

    // Detection bbox is `[x1, y1, x2, y2]` in image pixel space.
    let x1 = det.bbox[0].max(0.0).min(img_w as f32) as u32;
    let y1 = det.bbox[1].max(0.0).min(img_h as f32) as u32;
    let x2 = det.bbox[2].max(0.0).min(img_w as f32) as u32;
    let y2 = det.bbox[3].max(0.0).min(img_h as f32) as u32;
    if x2 <= x1 || y2 <= y1 {
        return;
    }
    let box_w = x2 - x1;
    let box_h = y2 - y1;

    for i in 0..LINE_THICKNESS {
        // `LINE_THICKNESS` nested rectangles inset by `i` pixels = a thick border.
        let off = i as i32;
        if box_w <= 2 * i || box_h <= 2 * i {
            break;
        }
        draw_hollow_rect_mut(
            img,
            Rect::at(x1 as i32 + off, y1 as i32 + off).of_size(box_w - 2 * i, box_h - 2 * i),
            BOX_COLOR,
        );
    }

    let pct = (det.confidence.clamp(0.0, 1.0) * 100.0).round() as u32;
    let label = format!("{} ({}%)", det.label, pct);
    let scale = PxScale {
        x: TEXT_PX,
        y: TEXT_PX,
    };
    let (text_w_i, text_h_i) = text_size(scale, font, &label);
    let text_w = text_w_i;
    let text_h = text_h_i;

    let bg_w = text_w + 2 * TEXT_PADDING_X;
    let bg_h = text_h + 2 * TEXT_PADDING_Y;

    // Anchor label at the top-right corner of the box if it fits there;
    // otherwise the top-left of the box.
    let bg_x = if box_w >= bg_w { x1 + box_w - bg_w } else { x1 };
    // Prefer above the box if there's room; otherwise below.
    let bg_y = if y1 >= bg_h {
        y1 - bg_h
    } else if y2 + bg_h <= img_h {
        y2
    } else {
        y1 // overlay on top edge as a last resort
    };

    draw_filled_rect_mut(
        img,
        Rect::at(bg_x as i32, bg_y as i32).of_size(bg_w, bg_h),
        TEXT_BG_COLOR,
    );
    draw_text_mut(
        img,
        TEXT_COLOR,
        (bg_x + TEXT_PADDING_X) as i32,
        (bg_y + TEXT_PADDING_Y) as i32,
        scale,
        font,
        &label,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg_blank(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::new_rgb8(w, h);
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg)
            .unwrap();
        out
    }

    fn det(bbox: [f32; 4], label: &str, conf: f32) -> Detection {
        Detection {
            bbox,
            label: label.into(),
            confidence: conf,
        }
    }

    #[test]
    fn draws_without_panic_on_normal_image() {
        let bytes = jpeg_blank(800, 600);
        let dets = vec![det([100.0, 100.0, 300.0, 250.0], "seal", 0.94)];
        let out = draw_bboxes(&bytes, &dets).unwrap();
        // Output decodes back as a valid image of the same dimensions.
        let decoded = ImageReader::new(Cursor::new(&out))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(decoded.width(), 800);
        assert_eq!(decoded.height(), 600);
    }

    #[test]
    fn empty_detection_list_returns_image() {
        let bytes = jpeg_blank(640, 480);
        let out = draw_bboxes(&bytes, &[]).unwrap();
        assert!(!out.is_empty());
    }

    #[test]
    fn rejects_oversized_image() {
        let bytes = {
            let img = DynamicImage::new_rgb8(MAX_IMAGE_DIM + 1, 100);
            let mut out = Vec::new();
            img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
                .unwrap();
            out
        };
        assert!(matches!(
            draw_bboxes(&bytes, &[]),
            Err(DrawError::ImageTooLarge { .. })
        ));
    }

    #[test]
    fn handles_bbox_outside_image() {
        // bbox extending outside the image bounds should be clipped, not panic.
        let bytes = jpeg_blank(200, 200);
        let dets = vec![det([-50.0, -50.0, 250.0, 250.0], "seal", 0.5)];
        let _ = draw_bboxes(&bytes, &dets).unwrap();
    }

    #[test]
    fn handles_zero_size_bbox() {
        // Degenerate bbox (x2 <= x1) should be silently skipped.
        let bytes = jpeg_blank(200, 200);
        let dets = vec![det([100.0, 100.0, 100.0, 100.0], "seal", 0.5)];
        let _ = draw_bboxes(&bytes, &dets).unwrap();
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            draw_bboxes(b"not an image", &[]),
            Err(DrawError::UnsupportedFormat | DrawError::Decode(_))
        ));
    }
}
