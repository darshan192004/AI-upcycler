//! Production bridge from a workflow step to generation.
//!
//! Routing decisions are delegated to sub-project A's [`RoutePolicy`] and
//! [`ProviderRegistry`]; this module owns no provider table, no secret store
//! and no API keys of its own.

use super::{StepFuture, StepOutput, StepRunner, WorkflowStep};
use crate::api::backend::{ChatMessage, GenerationBackend, GenerationRequest, MessageContent};
use crate::providers::{CloudRoute, ProviderRegistry};
use crate::routing::RoutePolicy;
use futures::StreamExt;
use std::sync::Arc;

/// The single production bridge from a workflow step to generation.
pub struct RouterBackend {
    backend: Arc<dyn GenerationBackend>,
    providers: Arc<ProviderRegistry>,
    model: String,
    session_id: String,
}

impl RouterBackend {
    pub fn new(
        backend: Arc<dyn GenerationBackend>,
        providers: Arc<ProviderRegistry>,
        model: String,
        session_id: String,
    ) -> Self {
        Self {
            backend,
            providers,
            model,
            session_id,
        }
    }

    fn route(&self, step: &WorkflowStep) -> Result<(RoutePolicy, Option<CloudRoute>), String> {
        let policy = RoutePolicy::parse(step.provider.as_deref())?;
        // The explicit provider token has already been classified by
        // `RoutePolicy`, so resolution stays model-driven. Passing the token
        // again would reject a local model id as a catalog miss before an
        // explicit cloud step can report that the provider has no route/key.
        let cloud = self.providers.resolve(&self.model, None)?;
        Ok((policy, cloud))
    }
}

impl StepRunner for RouterBackend {
    fn run<'a>(&'a self, step: &'a WorkflowStep) -> StepFuture<'a> {
        Box::pin(async move {
            let (policy, cloud) = self.route(step)?;
            let selected = match (policy, cloud) {
                (RoutePolicy::Local, _) => None,
                (RoutePolicy::Cloud(id), Some(route)) if route.provider == id => Some(route),
                (RoutePolicy::Cloud(id), Some(_)) => {
                    return Err(format!(
                        "workflow step '{}': model '{}' does not belong to provider '{}'",
                        step.id,
                        self.model,
                        id.as_str()
                    ));
                }
                (RoutePolicy::Cloud(id), None) => {
                    return Err(format!(
                        "workflow step '{}': provider '{}' is not configured for model '{}'",
                        step.id,
                        id.as_str(),
                        self.model
                    ));
                }
                (RoutePolicy::Auto, Some(route)) if !self.backend.is_real() => Some(route),
                (RoutePolicy::Auto, None) if !self.backend.is_real() => {
                    return Err(format!(
                        "workflow step '{}': no local engine and no cloud provider configured for model '{}'",
                        step.id, self.model
                    ));
                }
                (RoutePolicy::Auto, _) => None,
            };

            let generation_model = selected
                .as_ref()
                .map(|route| route.model.clone())
                .unwrap_or_else(|| self.model.clone());
            let request = GenerationRequest {
                model: generation_model,
                messages: vec![ChatMessage {
                    role: "user".into(),
                    content: MessageContent::Text(step.prompt.clone()),
                    ..Default::default()
                }],
                temperature: 0.2,
                max_tokens: 512,
                session_id: self.session_id.clone(),
                ..Default::default()
            };

            let mut stream = match selected {
                Some(route) => self.providers.stream(&route, request),
                None => self.backend.generate(request),
            };

            let mut content = String::new();
            let mut source = None;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                content.push_str(&chunk.content);
                if let Some(value) = chunk.source {
                    source = Some(value);
                }
            }

            Ok(StepOutput {
                content,
                source: source.unwrap_or_else(|| "local".into()),
            })
        })
    }
}
