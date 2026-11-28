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
    // The topology endpoint reads live host/phone memory at request time. The
    // telemetry socket carries health/temperature; this timer is what keeps the
    // RAM figures current even when no generation is running.
    const topologyTimer = window.setInterval(() => {
      void refreshTopology();
    }, 5000);
    const disconnect = connectTelemetry(applyTelemetry);
    return () => {
      window.clearInterval(topologyTimer);
      disconnect();
    };
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
