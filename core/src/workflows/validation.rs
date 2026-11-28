use super::executor::topological_order;
use super::WorkflowStep;
use std::collections::HashSet;

/// Trims ids/prompts/dependencies/provider tokens, checks referential
/// integrity, then proves the graph is acyclic. The returned vector keeps the
/// user's declaration order; execution order is derived separately.
pub fn validate_steps(steps: &[WorkflowStep]) -> Result<Vec<WorkflowStep>, String> {
    if steps.is_empty() {
        return Err("workflow must contain at least one step".into());
    }

    let mut normalized = Vec::with_capacity(steps.len());
    let mut seen = HashSet::new();

    for step in steps {
        let id = step.id.trim();
        if id.is_empty() {
            return Err("step id must not be empty".into());
        }
        if !seen.insert(id.to_string()) {
            return Err(format!("duplicate step id '{id}'"));
        }

        let prompt = step.prompt.trim();
        if prompt.is_empty() {
            return Err(format!("step '{id}' has an empty prompt"));
        }

        let mut depends_on = Vec::with_capacity(step.depends_on.len());
        let mut dep_seen = HashSet::new();
        for dep in &step.depends_on {
            let dep = dep.trim();
            if dep.is_empty() {
                return Err(format!("step '{id}' has an empty dependency"));
            }
            if dep == id {
                return Err(format!("step '{id}' cannot depend on itself"));
            }
            if !dep_seen.insert(dep.to_string()) {
                return Err(format!("step '{id}' repeats dependency '{dep}'"));
            }
            depends_on.push(dep.to_string());
        }

        let provider = step
            .provider
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if let Some(value) = &provider {
            crate::routing::RoutePolicy::parse(Some(value))
                .map_err(|error| format!("step '{id}': {error}"))?;
        }

        normalized.push(WorkflowStep {
            id: id.into(),
            prompt: prompt.into(),
            depends_on,
            provider,
        });
    }

    for step in &normalized {
        for dep in &step.depends_on {
            if !seen.contains(dep) {
                return Err(format!(
                    "step '{}' depends on unknown step '{dep}'",
                    step.id
                ));
            }
        }
    }

    topological_order(&normalized)?;
    Ok(normalized)
}
