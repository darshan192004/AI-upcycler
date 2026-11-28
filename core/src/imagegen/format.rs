use crate::imagegen::{GeneratedImageBytes, ImageError};

/// Detects PNG, JPEG, and WebP from magic bytes and returns the original bytes
/// with the matching MIME type and file extension.
pub fn sniff_image(bytes: &[u8]) -> Result<GeneratedImageBytes, ImageError> {
    const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    const JPEG_MAGIC: [u8; 3] = [0xFF, 0xD8, 0xFF];

    let (mime_type, extension) = if bytes.starts_with(&PNG_MAGIC) {
        ("image/png", "png")
    } else if bytes.starts_with(&JPEG_MAGIC) {
        ("image/jpeg", "jpg")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        ("image/webp", "webp")
    } else {
        return Err(ImageError::UnsupportedFormat);
    };

    Ok(GeneratedImageBytes {
        bytes: bytes.to_vec(),
        mime_type: mime_type.to_string(),
        extension: extension.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_1X1_BASE64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p8kAAAAASUVORK5CYII=";

    #[test]
    fn sniffs_png_jpeg_and_webp() {
        let png =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, PNG_1X1_BASE64)
                .unwrap();
        assert_eq!(sniff_image(&png).unwrap().mime_type, "image/png");
        assert_eq!(sniff_image(&png).unwrap().extension, "png");

        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        assert_eq!(sniff_image(&jpeg).unwrap().mime_type, "image/jpeg");
        assert_eq!(sniff_image(&jpeg).unwrap().extension, "jpg");

        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0x1A, 0x00, 0x00, 0x00]);
        webp.extend_from_slice(b"WEBPVP8 ");
        assert_eq!(sniff_image(&webp).unwrap().mime_type, "image/webp");
        assert_eq!(sniff_image(&webp).unwrap().extension, "webp");
    }

    #[test]
    fn rejects_unknown_bytes() {
        assert!(matches!(
            sniff_image(b"not an image"),
            Err(ImageError::UnsupportedFormat)
        ));
    }
}
