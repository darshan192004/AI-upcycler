//! OpenAI adapter. Its wire protocol is the shared OpenAI-compatible helper.

use super::openai_compat::OpenAiCompatProvider;
use super::ProviderId;

pub(crate) fn provider(client: reqwest::Client, base_url: String) -> OpenAiCompatProvider {
    OpenAiCompatProvider::new(ProviderId::OpenAI, client, base_url)
}
