//! Provider message mapping.
//!
//! These mappers are pure: they never hold image bytes. Callers supply the
//! encoded form each provider needs (a data URL for OpenAI-compatible
//! providers, raw base64 for Gemini and Anthropic).

use crate::api::backend::{ChatMessage, ContentPart, MessageContent};
use serde_json::Value;

/// Maps one chat message to an OpenAI-compatible wire message. Kimi reuses
/// this shape. `data_url` is caller-supplied (`data:<mime>;base64,<payload>`);
/// no image bytes ever pass through this mapper.
pub fn to_openai_message(msg: &ChatMessage, data_url: &str) -> Value {
    match &msg.content {
        // Legacy text-only turns keep A's plain-string content shape.
        MessageContent::Text(text) => {
            serde_json::json!({"role": &msg.role, "content": text})
        }
        MessageContent::Parts(parts) => {
            let content: Vec<Value> = parts
                .iter()
                .map(|part| match part {
                    ContentPart::Text { text } => {
                        serde_json::json!({"type": "text", "text": text})
                    }
                    ContentPart::Image { .. } => serde_json::json!({
                        "type": "image_url",
                        "image_url": {"url": data_url},
                    }),
                })
                .collect();
            serde_json::json!({"role": &msg.role, "content": content})
        }
    }
}

/// Maps one chat message to Gemini `parts`. `base64_data` is caller-supplied
/// raw base64 (the portion after the comma of a data URL); no image bytes ever
/// pass through this mapper.
pub fn to_gemini_parts(msg: &ChatMessage, base64_data: &str) -> Vec<Value> {
    match &msg.content {
        MessageContent::Text(text) => vec![serde_json::json!({"text": text})],
        MessageContent::Parts(parts) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => serde_json::json!({"text": text}),
                ContentPart::Image { mime_type, .. } => serde_json::json!({
                    "inlineData": {"mimeType": mime_type, "data": base64_data},
                }),
            })
            .collect(),
    }
}

/// Maps one chat message to Anthropic content blocks. `base64_data` is
/// caller-supplied raw base64; no image bytes ever pass through this mapper.
pub fn to_anthropic_content(msg: &ChatMessage, base64_data: &str) -> Vec<Value> {
    match &msg.content {
        MessageContent::Text(text) => vec![serde_json::json!({"type": "text", "text": text})],
        MessageContent::Parts(parts) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => {
                    serde_json::json!({"type": "text", "text": text})
                }
                ContentPart::Image { mime_type, .. } => serde_json::json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": mime_type,
                        "data": base64_data,
                    },
                }),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::backend::{ChatMessage, ContentPart, MessageContent};

    fn image_msg() -> ChatMessage {
        ChatMessage {
            role: "user".into(),
            content: MessageContent::Parts(vec![ContentPart::Image {
                media_hash: "ab".repeat(32),
                mime_type: "image/png".into(),
                file_name: "a.png".into(),
                byte_count: 4,
            }]),
            ..Default::default()
        }
    }

    #[test]
    fn openai_maps_image_to_data_url() {
        let v = to_openai_message(&image_msg(), "data:image/png;base64,QUJD");
        assert_eq!(v["content"][0]["type"], "image_url");
        assert_eq!(
            v["content"][0]["image_url"]["url"],
            "data:image/png;base64,QUJD"
        );
    }

    #[test]
    fn gemini_maps_image_to_inline_data() {
        let v = to_gemini_parts(&image_msg(), "QUJD");
        assert_eq!(v[0]["inlineData"]["mimeType"], "image/png");
        assert_eq!(v[0]["inlineData"]["data"], "QUJD");
    }

    #[test]
    fn anthropic_maps_image_to_source_base64() {
        let v = to_anthropic_content(&image_msg(), "QUJD");
        assert_eq!(v[0]["type"], "image");
        assert_eq!(v[0]["source"]["type"], "base64");
        assert_eq!(v[0]["source"]["media_type"], "image/png");
        assert_eq!(v[0]["source"]["data"], "QUJD");
    }
}
