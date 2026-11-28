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

/** Only the newest selection may write `selected`, including repeat clicks on one run. */
let selectionGeneration = 0;

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
    const generation = ++selectionGeneration;
    try {
      const selected = await getWorkflow(runId);
      if (generation !== selectionGeneration) return;
      set({ selected, error: null });
    } catch (error) {
      if (generation !== selectionGeneration) return;
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
    const status: WorkflowStepStatus =
      event.event === 'workflow_step_started' ? 'running' : event.data.status;
    const runs = get().runs;
    const selected = get().selected;
    const hasHistoryRow = runs.some((run) => run.run_id === runId);
    if (!hasHistoryRow && selected?.run.run_id !== runId) return;

    let nextSelected = selected;
    let selectedStatus: WorkflowRunRow['status'] | null = null;
    if (selected?.run.run_id === runId) {
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
      selectedStatus = deriveRunStatus(steps);
      nextSelected = {
        run: { ...selected.run, status: selectedStatus },
        steps,
      };
    }

    const historyStatus: WorkflowRunRow['status'] | null =
      selectedStatus ??
      (event.event === 'workflow_step_started' || status === 'running'
        ? 'running'
        : status === 'failed' || status === 'blocked'
          ? 'failed'
          : null);
    set({
      ...(nextSelected ? { selected: nextSelected } : {}),
      ...(hasHistoryRow && historyStatus
        ? {
            runs: runs.map((run) =>
              run.run_id === runId ? { ...run, status: historyStatus } : run,
            ),
          }
        : {}),
    });

    // A completed step may finish the whole run; refresh the authoritative
    // history even when the event belongs to a run that is not selected.
    if (hasHistoryRow && event.event === 'workflow_step_completed') {
      void get().refreshRuns();
    }
  },
}));
