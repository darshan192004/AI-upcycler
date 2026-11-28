//! Task 1 stub: validation needs cycle detection before the executor lands.
use super::WorkflowStep;
use std::collections::{HashMap, VecDeque};

pub(crate) fn topological_order(steps: &[WorkflowStep]) -> Result<Vec<usize>, String> {
    let mut by_id = HashMap::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        if by_id.insert(step.id.as_str(), index).is_some() {
            return Err(format!("duplicate step id '{}'", step.id));
        }
    }

    let mut indegree = vec![0usize; steps.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); steps.len()];
    for (index, step) in steps.iter().enumerate() {
        for dep in &step.depends_on {
            let dep_index = *by_id
                .get(dep.as_str())
                .ok_or_else(|| format!("step '{}' depends on unknown step '{dep}'", step.id))?;
            indegree[index] += 1;
            dependents[dep_index].push(index);
        }
    }

    let mut ready: VecDeque<usize> = (0..steps.len()).filter(|&i| indegree[i] == 0).collect();
    let mut order = Vec::with_capacity(steps.len());
    while let Some(index) = ready.pop_front() {
        order.push(index);
        for &next in &dependents[index] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.push_back(next);
            }
        }
    }

    if order.len() != steps.len() {
        return Err("workflow contains a dependency cycle".into());
    }
    Ok(order)
}
