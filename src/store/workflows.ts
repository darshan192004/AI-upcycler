import { create } from 'zustand';
import { createWorkflow, getWorkflow, listWorkflows } from '../lib/api';
import type {
  CreateWorkflowResponse,
  TelemetryEvent,
  WorkflowRequest,
  WorkflowRunDetail,
  WorkflowRunRow,
  WorkflowStepRow,
  WorkflowStepStatus,
} from '../lib/types';

function deriveRunStatus(steps: WorkflowStepRow[]): WorkflowRunRow['status'] {
  if (steps.some((step) => step.status === 'failed' || step.status === 'blocked')) return 'failed';
  if (steps.length > 0 && steps.every((step) => step.status === 'completed')) return 'completed';
  return 'running';
}

interface WorkflowState {
  runs: WorkflowRunRow[];
  selected: WorkflowRunDetail | null;
  error: string | null;
  refreshRuns: () => Promise<void>;
  selectRun: (runId: string) => Promise<void>;
  startWorkflow: (req: WorkflowRequest) => Promise<CreateWorkflowResponse>;
  applyWorkflowTelemetry: (event: TelemetryEvent) => void;
}

export const useWorkflowStore = create<WorkflowState>((set, get) => ({
  runs: [],
  selected: null,
  error: null,

  refreshRuns: async () => {
    try {
      const { runs } = await listWorkflows();
      set({ runs, error: null });
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error) });
    }
  },

  selectRun: async (runId) => {
    try {
      const selected = await getWorkflow(runId);
      set({ selected, error: null });
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error) });
    }
  },

  startWorkflow: async (req) => {
    try {
      const created = await createWorkflow(req);
      set({ error: null });
      await get().refreshRuns();
      await get().selectRun(created.run_id);
      return created;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      set({ error: message });
      throw new Error(message);
    }
  },

  applyWorkflowTelemetry: (event) => {
    if (
      event.event !== 'workflow_step_started' &&
      event.event !== 'workflow_step_completed'
    ) {
      return;
    }
    const { run_id: runId, step_id: stepId } = event.data;
    const selected = get().selected;
    if (!selected || selected.run.run_id !== runId) return;

    const status: WorkflowStepStatus =
      event.event === 'workflow_step_started' ? 'running' : event.data.status;
    const steps = selected.steps.map((step): WorkflowStepRow => {
      if (step.step_id !== stepId) return step;
      if (event.event === 'workflow_step_started') return { ...step, status };
      return {
        ...step,
        status,
        output: event.data.output,
        source: event.data.source,
        error: event.data.error,
      };
    });
    set({
      selected: {
        run: { ...selected.run, status: deriveRunStatus(steps) },
        steps,
      },
    });
  },
}));
