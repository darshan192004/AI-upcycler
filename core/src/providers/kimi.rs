//! Kimi (Moonshot) adapter. It speaks OpenAI's `/chat/completions` protocol
//! and resolves image parts through the same media-aware payload builder as
//! OpenAI.

use super::openai::MediaOpenAiCompatProvider;
use super::ProviderId;
use crate::media::MediaStore;
use std::sync::Arc;

pub(crate) fn provider(
    client: reqwest::Client,
    base_url: String,
    media: Option<Arc<MediaStore>>,
) -> MediaOpenAiCompatProvider {
    MediaOpenAiCompatProvider::new(ProviderId::Kimi, client, base_url, media)
}
