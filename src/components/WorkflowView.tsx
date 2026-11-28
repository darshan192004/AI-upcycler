import { useRef, useState } from 'react';
import {
  AlertTriangle,
  Ban,
  Check,
  Circle,
  Loader2,
  Plus,
  Trash2,
} from 'lucide-react';
import { useClusterStore } from '../store/cluster';
import { useWorkflowStore } from '../store/workflows';
import type {
  WorkflowProvider,
  WorkflowRunDetail,
  WorkflowStepDraft,
  WorkflowStepStatus,
} from '../lib/types';

const PROVIDERS: WorkflowProvider[] = ['auto', 'local', 'openai', 'anthropic', 'gemini', 'kimi'];

const STATUS_LABEL: Record<WorkflowStepStatus, string> = {
  pending: 'Pending',
  running: 'Running',
  completed: 'Completed',
  failed: 'Failed',
  blocked: 'Blocked',
};

function StatusIcon({ status }: { status: WorkflowStepStatus }) {
  const props = { className: 'wf-status-icon', 'aria-hidden': true } as const;
  switch (status) {
    case 'completed':
      return <Check {...props} />;
    case 'running':
      return <Loader2 {...props} />;
    case 'failed':
      return <AlertTriangle {...props} />;
    case 'blocked':
      return <Ban {...props} />;
    case 'pending':
      return <Circle {...props} />;
  }
}

export function WorkflowView() {
  const models = useClusterStore((state) => state.models);
  const runs = useWorkflowStore((state) => state.runs);
  const selected = useWorkflowStore((state) => state.selected);
  const error = useWorkflowStore((state) => state.error);
  const refreshRuns = useWorkflowStore((state) => state.refreshRuns);
  const selectRun = useWorkflowStore((state) => state.selectRun);
  const startWorkflow = useWorkflowStore((state) => state.startWorkflow);

  const [model, setModel] = useState('');
  const [steps, setSteps] = useState<WorkflowStepDraft[]>(() => [
    { id: 's1', prompt: '', depends_on: [], provider: 'auto' },
  ]);
  const nextId = useRef(2);

  const updateStep = (id: string, patch: Partial<WorkflowStepDraft>) => {
    setSteps((current) =>
      current.map((step) => (step.id === id ? { ...step, ...patch } : step)),
    );
  };

  const addStep = () => {
    const id = `s${nextId.current++}`;
    setSteps((current) => [
      ...current,
      { id, prompt: '', depends_on: [], provider: 'auto' },
    ]);
  };

  const removeStep = (id: string) => {
    setSteps((current) =>
      current
        .filter((step) => step.id !== id)
        .map((step) => ({
          ...step,
          depends_on: step.depends_on.filter((dep) => dep !== id),
        })),
    );
  };

  const submit = async () => {
    await startWorkflow({ model, steps });
  };

  return (
    <section className="workflow-view" aria-labelledby="workflow-heading">
      <header className="wf-header">
        <h2 id="workflow-heading">Task workflow</h2>
        <button type="button" className="wf-button !min-h-12" onClick={() => void refreshRuns()}>
          Refresh runs
        </button>
      </header>

      {error && <p className="wf-error" role="alert">{error}</p>}

      <form
        className="wf-editor"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <label className="wf-field">
          <span>Model</span>
          <select
            className="!min-h-12"
            value={model}
            onChange={(event) => setModel(event.target.value)}
            disabled={models.length === 0}
            required
          >
            <option value="">Select a model</option>
            {models.map((entry) => (
              <option key={entry.id} value={entry.id}>{entry.id}</option>
            ))}
          </select>
        </label>
        {models.length === 0 && <p className="wf-hint">Load a model on the Models tab first.</p>}

        <ol className="wf-step-list">
          {steps.map((step) => (
            <li key={step.id} className="wf-step-editor">
              <div className="wf-step-row">
                <span className="wf-step-id">{step.id}</span>
                <label className="wf-field wf-grow">
                  <span>Prompt</span>
                  <input
                    className="!min-h-12"
                    type="text"
                    value={step.prompt}
                    onChange={(event) => updateStep(step.id, { prompt: event.target.value })}
                    required
                  />
                </label>
                <label className="wf-field">
                  <span>Provider</span>
                  <select
                    className="!min-h-12"
                    value={step.provider}
                    onChange={(event) =>
                      updateStep(step.id, { provider: event.target.value as WorkflowProvider })
                    }
                  >
                    {PROVIDERS.map((provider) => (
                      <option key={provider} value={provider}>{provider}</option>
                    ))}
                  </select>
                </label>
                <label className="wf-field">
                  <span>Depends on</span>
                  <select
                    className="!min-h-12"
                    multiple
                    value={step.depends_on}
                    onChange={(event) =>
                      updateStep(step.id, {
                        depends_on: Array.from(event.target.selectedOptions, (option) => option.value),
                      })
                    }
                  >
                    {steps
                      .filter((candidate) => candidate.id !== step.id)
                      .map((candidate) => (
                        <option key={candidate.id} value={candidate.id}>{candidate.id}</option>
                      ))}
                  </select>
                </label>
                <button
                  type="button"
                  className="wf-button wf-danger !min-h-12"
                  aria-label={`Remove step ${step.id}`}
                  onClick={() => removeStep(step.id)}
                >
                  <Trash2 className="wf-status-icon" aria-hidden="true" />
                  Remove
                </button>
              </div>
            </li>
          ))}
        </ol>

        <div className="wf-actions">
          <button type="button" className="wf-button !min-h-12" onClick={addStep}>
            <Plus className="wf-status-icon" aria-hidden="true" />
            Add step
          </button>
          <button type="submit" className="wf-button wf-primary !min-h-12" disabled={!model}>
            Start run
          </button>
        </div>
      </form>

      <section className="wf-history" aria-labelledby="workflow-runs-heading">
        <h3 id="workflow-runs-heading">Runs</h3>
        <ul className="wf-run-list">
          {runs.map((run) => (
            <li key={run.run_id}>
              <button
                type="button"
                className="wf-button wf-run !min-h-12"
                aria-current={selected?.run.run_id === run.run_id}
                onClick={() => void selectRun(run.run_id)}
              >
                {run.run_id} · {run.model} · {run.status}
              </button>
            </li>
          ))}
        </ul>
      </section>

      {selected && <WorkflowTimeline detail={selected} />}
    </section>
  );
}

function WorkflowTimeline({ detail }: { detail: WorkflowRunDetail }) {
  return (
    <section className="wf-timeline" aria-labelledby="workflow-timeline-heading">
      <h3 id="workflow-timeline-heading">Timeline · {detail.run.run_id}</h3>
      <ol className="wf-timeline-list">
        {detail.steps.map((step) => (
          <li
            key={step.step_id}
            className="wf-timeline-step"
            aria-busy={step.status === 'running'}
            aria-current={step.status === 'running' ? 'step' : undefined}
          >
            <span className={`wf-status wf-status-${step.status}`}>
              <StatusIcon status={step.status} />
              {STATUS_LABEL[step.status]}
            </span>
            <strong>{step.step_id}</strong>
            <span className="wf-muted">{step.prompt}</span>
            {step.source && <span className="wf-muted">source: {step.source}</span>}
            {step.output && <p className="wf-output">{step.output}</p>}
            {step.error && <p className="wf-error">{step.error}</p>}
          </li>
        ))}
      </ol>
    </section>
  );
}

export default WorkflowView;
