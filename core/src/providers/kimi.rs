//! Kimi (Moonshot) adapter. It speaks OpenAI's `/chat/completions` protocol.

use super::openai_compat::OpenAiCompatProvider;
use super::ProviderId;

pub(crate) fn provider(client: reqwest::Client, base_url: String) -> OpenAiCompatProvider {
    OpenAiCompatProvider::new(ProviderId::Kimi, client, base_url)
}
