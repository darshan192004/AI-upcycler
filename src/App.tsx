import React, { useState } from 'react';
import { Cpu, HardDrive, Smartphone, Zap, ShieldCheck, Activity } from 'lucide-react';

export const App: React.FC = () => {
  const [activeTab, setActiveTab] = useState<'dashboard' | 'nodes' | 'models' | 'chat'>('dashboard');

  return (
    <div className="min-h-screen flex flex-col bg-[#0B0F19] text-gray-100 font-sans">
      {/* Top Header Bar */}
      <header className="h-16 border-b border-gray-800/80 px-6 flex items-center justify-between glass-panel sticky top-0 z-50">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 rounded-xl bg-emerald-500/10 border border-emerald-500/30 flex items-center justify-center text-emerald-400 font-bold text-xl">
            ♻️
          </div>
          <div>
            <h1 className="font-bold text-lg leading-none tracking-tight">AI Upcycler</h1>
            <p className="text-xs text-gray-400 mt-1">Distributed Hardware Pooling Cluster</p>
          </div>
        </div>

        <div className="flex items-center gap-4">
          <div className="flex items-center gap-2 px-3 py-1.5 rounded-full bg-emerald-500/10 border border-emerald-500/30 text-emerald-400 text-xs font-medium">
            <span className="w-2 h-2 rounded-full bg-emerald-400 animate-pulse"></span>
            Cluster Status: ACTIVE (16 GB Pooled)
          </div>
          <div className="text-xs text-gray-400 flex items-center gap-1">
            <ShieldCheck className="w-4 h-4 text-emerald-400" />
            mTLS Encrypted
          </div>
        </div>
      </header>

      {/* Main Content Layout */}
      <div className="flex flex-1">
        {/* Navigation Sidebar */}
        <aside className="w-64 border-r border-gray-800/80 p-4 flex flex-col gap-2 bg-[#0B0F19]/50">
          <button
            onClick={() => setActiveTab('dashboard')}
            className={`flex items-center gap-3 px-4 py-3 rounded-xl text-sm font-medium transition-all ${
              activeTab === 'dashboard'
                ? 'bg-emerald-500/10 text-emerald-400 border border-emerald-500/30'
                : 'text-gray-400 hover:text-gray-200 hover:bg-gray-800/40'
            }`}
          >
            <Activity className="w-4 h-4" />
            Cluster Dashboard
          </button>
          <button
            onClick={() => setActiveTab('nodes')}
            className={`flex items-center gap-3 px-4 py-3 rounded-xl text-sm font-medium transition-all ${
              activeTab === 'nodes'
                ? 'bg-emerald-500/10 text-emerald-400 border border-emerald-500/30'
                : 'text-gray-400 hover:text-gray-200 hover:bg-gray-800/40'
            }`}
          >
            <Smartphone className="w-4 h-4" />
            Pooled Devices (3 Nodes)
          </button>
          <button
            onClick={() => setActiveTab('models')}
            className={`flex items-center gap-3 px-4 py-3 rounded-xl text-sm font-medium transition-all ${
              activeTab === 'models'
                ? 'bg-emerald-500/10 text-emerald-400 border border-emerald-500/30'
                : 'text-gray-400 hover:text-gray-200 hover:bg-gray-800/40'
            }`}
          >
            <HardDrive className="w-4 h-4" />
            GGUF Model Catalog
          </button>
          <button
            onClick={() => setActiveTab('chat')}
            className={`flex items-center gap-3 px-4 py-3 rounded-xl text-sm font-medium transition-all ${
              activeTab === 'chat'
                ? 'bg-emerald-500/10 text-emerald-400 border border-emerald-500/30'
                : 'text-gray-400 hover:text-gray-200 hover:bg-gray-800/40'
            }`}
          >
            <Zap className="w-4 h-4" />
            Local LLM Chat
          </button>
        </aside>

        {/* Dashboard Workspace */}
        <main className="flex-1 p-8 overflow-y-auto">
          <div className="max-w-6xl mx-auto space-y-6">
            {/* Top Stat Cards */}
            <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
              <div className="glass-panel p-5 rounded-2xl border border-gray-800">
                <div className="flex items-center justify-between text-gray-400 mb-2">
                  <span className="text-xs uppercase tracking-wider font-semibold">Total Pooled RAM</span>
                  <Cpu className="w-4 h-4 text-emerald-400" />
                </div>
                <div className="text-2xl font-bold text-gray-100">16.0 GB</div>
                <div className="text-xs text-emerald-400 mt-1">Host 8GB + 2x 4GB Phones</div>
              </div>

              <div className="glass-panel p-5 rounded-2xl border border-gray-800">
                <div className="flex items-center justify-between text-gray-400 mb-2">
                  <span className="text-xs uppercase tracking-wider font-semibold">Active Model</span>
                  <HardDrive className="w-4 h-4 text-cyan-400" />
                </div>
                <div className="text-lg font-bold text-gray-100 truncate">Qwen2.5-14B Q4</div>
                <div className="text-xs text-gray-400 mt-1">48 Layers Split Across 3 Nodes</div>
              </div>

              <div className="glass-panel p-5 rounded-2xl border border-gray-800">
                <div className="flex items-center justify-between text-gray-400 mb-2">
                  <span className="text-xs uppercase tracking-wider font-semibold">Generation Speed</span>
                  <Zap className="w-4 h-4 text-amber-400" />
                </div>
                <div className="text-2xl font-bold text-gray-100">8.4 tok/s</div>
                <div className="text-xs text-amber-400 mt-1">USB 3.0 Tethered Pipeline</div>
              </div>

              <div className="glass-panel p-5 rounded-2xl border border-gray-800">
                <div className="flex items-center justify-between text-gray-400 mb-2">
                  <span className="text-xs uppercase tracking-wider font-semibold">SSD mmap Tier</span>
                  <Activity className="w-4 h-4 text-emerald-400" />
                </div>
                <div className="text-2xl font-bold text-emerald-400">READY</div>
                <div className="text-xs text-gray-400 mt-1">NVMe 5000 MB/s Page Buffer</div>
              </div>
            </div>

            {/* Layer Topology Visualizer */}
            <div className="glass-panel p-6 rounded-2xl border border-gray-800 space-y-4">
              <h2 className="text-sm font-semibold text-gray-300 uppercase tracking-wider">
                Live Pipelined Layer Partitioning
              </h2>
              <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                <div className="p-4 rounded-xl bg-gray-900/80 border border-emerald-500/30">
                  <div className="flex items-center justify-between mb-2">
                    <span className="font-semibold text-gray-100 text-sm">Laptop Host (Master)</span>
                    <span className="text-xs text-emerald-400 font-mono">Layers 0 → 16</span>
                  </div>
                  <div className="w-full bg-gray-800 h-2 rounded-full overflow-hidden">
                    <div className="bg-emerald-500 h-full w-[65%]"></div>
                  </div>
                  <p className="text-xs text-gray-400 mt-2">RAM Assigned: 4.2 GB / 8.0 GB</p>
                </div>

                <div className="p-4 rounded-xl bg-gray-900/80 border border-emerald-500/30">
                  <div className="flex items-center justify-between mb-2">
                    <span className="font-semibold text-gray-100 text-sm">Galaxy S10 (USB Phone 1)</span>
                    <span className="text-xs text-emerald-400 font-mono">Layers 17 → 32</span>
                  </div>
                  <div className="w-full bg-gray-800 h-2 rounded-full overflow-hidden">
                    <div className="bg-emerald-500 h-full w-[80%]"></div>
                  </div>
                  <p className="text-xs text-gray-400 mt-2">RAM Assigned: 3.2 GB / 4.0 GB (1.2 ms Ping)</p>
                </div>

                <div className="p-4 rounded-xl bg-gray-900/80 border border-emerald-500/30">
                  <div className="flex items-center justify-between mb-2">
                    <span className="font-semibold text-gray-100 text-sm">Pixel 4a (Wi-Fi Phone 2)</span>
                    <span className="text-xs text-emerald-400 font-mono">Layers 33 → 48</span>
                  </div>
                  <div className="w-full bg-gray-800 h-2 rounded-full overflow-hidden">
                    <div className="bg-emerald-500 h-full w-[78%]"></div>
                  </div>
                  <p className="text-xs text-gray-400 mt-2">RAM Assigned: 3.1 GB / 4.0 GB (8.4 ms Ping)</p>
                </div>
              </div>
            </div>
          </div>
        </main>
      </div>
    </div>
  );
};

export default App;
