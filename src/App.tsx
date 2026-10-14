import { useEffect, useState } from 'react';
import { AppShell, type AppTab } from './components/AppShell';
import { ChatView } from './components/ChatView';
import { DashboardView } from './components/DashboardView';
import { ModelsView } from './components/ModelsView';
import { connectTelemetry } from './lib/telemetry';
import { useClusterStore } from './store/cluster';

export const App: React.FC = () => {
  const [activeTab, setActiveTab] = useState<AppTab>('dashboard');
  const refreshTopology = useClusterStore((s) => s.refreshTopology);
  const refreshModels = useClusterStore((s) => s.refreshModels);
  const applyTelemetry = useClusterStore((s) => s.applyTelemetry);

  useEffect(() => {
    void refreshTopology();
    void refreshModels();
    const disconnect = connectTelemetry(applyTelemetry);
    return disconnect;
  }, [refreshTopology, refreshModels, applyTelemetry]);

  return (
    <AppShell activeTab={activeTab} onTabChange={setActiveTab}>
      {activeTab === 'dashboard' && <DashboardView />}
      {activeTab === 'models' && <ModelsView />}
      {activeTab === 'chat' && <ChatView />}
    </AppShell>
  );
};

export default App;
