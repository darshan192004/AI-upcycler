import { useEffect, useRef } from 'react';
import { motion, useReducedMotion } from 'motion/react';
import Lenis from 'lenis';
import {
  Activity,
  HardDrive,
  LayoutDashboard,
  MessageSquare,
  Recycle,
  Smartphone,
} from 'lucide-react';
import { useClusterStore } from '../store/cluster';
import { AuroraBackground } from './ui/AuroraBackground';
import { SegmentedTabs, type TabItem } from './ui/SegmentedTabs';
import { StatusDot } from './ui/StatusDot';

export type AppTab = 'dashboard' | 'models' | 'chat';

const NAV: TabItem<AppTab>[] = [
  { id: 'dashboard', label: 'Dashboard', icon: <LayoutDashboard className="h-4 w-4" /> },
  { id: 'models', label: 'Models', icon: <HardDrive className="h-4 w-4" /> },
  { id: 'chat', label: 'Chat', icon: <MessageSquare className="h-4 w-4" /> },
];

interface AppShellProps {
  activeTab: AppTab;
  onTabChange: (tab: AppTab) => void;
  children: React.ReactNode;
}

export function AppShell({ activeTab, onTabChange, children }: AppShellProps) {
  const topology = useClusterStore((s) => s.topology);
  const apiOnline = useClusterStore((s) => s.apiOnline);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const reduceMotion = useReducedMotion();

  useEffect(() => {
    const wrapper = scrollRef.current;
    const content = contentRef.current;
    if (!wrapper || !content || reduceMotion) return;

    const lenis = new Lenis({ wrapper, content, smoothWheel: true });
    let frame = 0;
    const raf = (time: number) => {
      lenis.raf(time);
      frame = requestAnimationFrame(raf);
    };
    frame = requestAnimationFrame(raf);
    return () => {
      cancelAnimationFrame(frame);
      lenis.destroy();
    };
  }, [reduceMotion]);

  const pooledGb = topology ? topology.total_pooled_ram_mb / 1024 : 0;
  const devices = topology?.nodes.filter((n) => n.role !== 'master').length ?? 0;

  return (
    <div className="relative flex h-screen flex-col overflow-hidden">
      <AuroraBackground />

      <header className="glass-panel z-40 flex h-16 shrink-0 items-center justify-between border-b border-white/8 px-5">
        <div className="flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl border border-emerald-400/30 bg-emerald-500/10 text-emerald-300">
            <Recycle className="h-5 w-5" aria-hidden="true" />
          </div>
          <div>
            <h1 className="text-lg font-bold leading-none tracking-tight">
              <span className="text-gradient">AI Upcycler</span>
            </h1>
            <p className="mt-1 text-[11px] text-slate-400">
              distributed hardware pooling cluster
            </p>
          </div>
        </div>

        <div className="flex items-center gap-2.5">
          <div className="hidden items-center gap-2 rounded-full border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-300 sm:flex">
            <Smartphone className="h-3.5 w-3.5 text-cyan-400" />
            <span className="font-mono">
              {devices} device{devices === 1 ? '' : 's'}
            </span>
          </div>
          <div className="hidden items-center gap-2 rounded-full border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-300 md:flex">
            <Activity className="h-3.5 w-3.5 text-amber-400" />
            <span className="font-mono">{pooledGb.toFixed(1)} GB pooled</span>
          </div>
          <div
            className={`flex items-center gap-2 rounded-full border px-3 py-1.5 text-xs font-medium ${
              apiOnline
                ? 'border-emerald-400/30 bg-emerald-500/10 text-emerald-300'
                : 'border-red-400/30 bg-red-500/10 text-red-300'
            }`}
          >
            <StatusDot tone={apiOnline ? 'ok' : 'error'} />
            {apiOnline ? (topology?.status ?? 'online') : 'API offline'}
          </div>
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        <aside className="hidden w-56 shrink-0 flex-col gap-1 border-r border-white/8 p-3 md:flex">
          <SegmentedTabs
            items={NAV}
            active={activeTab}
            onChange={onTabChange}
            layoutId="sidebar-pill"
            className="flex-col items-stretch !bg-transparent !p-0 !border-0"
          />
          <div className="mt-auto rounded-xl border border-white/8 bg-white/4 p-3">
            <p className="text-[10px] font-semibold uppercase tracking-wider text-slate-500">
              transport
            </p>
            <p className="mt-1 text-xs text-slate-400">
              Loopback API · mTLS worker RPC
            </p>
            <p className="mt-1 text-[10px] text-slate-600">
              USB plaintext until paired (§3)
            </p>
          </div>
        </aside>

        <div className="flex min-w-0 flex-1 flex-col">
          <div className="border-b border-white/8 p-3 md:hidden">
            <SegmentedTabs
              items={NAV}
              active={activeTab}
              onChange={onTabChange}
              layoutId="mobile-pills"
              className="w-full justify-between"
            />
          </div>

          <div
            ref={scrollRef}
            className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden"
          >
            <div ref={contentRef} className="mx-auto w-full max-w-6xl px-5 py-6">
              <motion.div
                key={activeTab}
                initial={{ opacity: 0, y: 10 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ duration: reduceMotion ? 0 : 0.28, ease: 'easeOut' }}
              >
                {children}
              </motion.div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
