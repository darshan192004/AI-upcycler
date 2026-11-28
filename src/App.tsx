import { useEffect, useState } from 'react';
import { AppShell, type AppTab } from './components/AppShell';
import { ChatView } from './components/ChatView';
import { DashboardView } from './components/DashboardView';
import { ModelsView } from './components/ModelsView';
import { WorkflowView } from './components/WorkflowView';
import { connectTelemetry } from './lib/telemetry';
import { useClusterStore } from './store/cluster';
import { useWorkflowStore } from './store/workflows';

export const App: React.FC = () => {
  const [activeTab, setActiveTab] = useState<AppTab>('dashboard');
  const refreshTopology = useClusterStore((s) => s.refreshTopology);
  const refreshModels = useClusterStore((s) => s.refreshModels);
  const applyTelemetry = useClusterStore((s) => s.applyTelemetry);
  const applyWorkflowTelemetry = useWorkflowStore((s) => s.applyWorkflowTelemetry);
  const refreshRuns = useWorkflowStore((s) => s.refreshRuns);

  useEffect(() => {
    void refreshTopology();
    void refreshModels();
    void refreshRuns();
    // Topology reads live host/phone memory at request time; the telemetry
    // socket carries health/temperature and workflow step transitions.
    const topologyTimer = window.setInterval(() => {
      void refreshTopology();
    }, 5000);
    const disconnectCluster = connectTelemetry(applyTelemetry);
    const disconnectWorkflows = connectTelemetry(applyWorkflowTelemetry);
    return () => {
      window.clearInterval(topologyTimer);
      disconnectCluster();
      disconnectWorkflows();
    };
  }, [refreshTopology, refreshModels, refreshRuns, applyTelemetry, applyWorkflowTelemetry]);

  return (
    <AppShell activeTab={activeTab} onTabChange={setActiveTab}>
      {activeTab === 'dashboard' && <DashboardView />}
      {activeTab === 'models' && <ModelsView />}
      {activeTab === 'chat' && <ChatView />}
      {activeTab === 'workflows' && <WorkflowView />}
    </AppShell>
  );
};

export default App;
