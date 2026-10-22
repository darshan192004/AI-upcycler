import { motion } from 'motion/react';
import { Activity, Cpu, HardDrive, Zap } from 'lucide-react';
import { useClusterStore } from '../store/cluster';
import { GlowCard } from './ui/GlowCard';
import { NumberTicker } from './ui/NumberTicker';
import { Sparkline } from './ui/Sparkline';
import { StatusDot } from './ui/StatusDot';
import { NodeCard } from './NodeCard';
import { PairingPanel } from './PairingPanel';

const container = {
  hidden: {},
  show: { transition: { staggerChildren: 0.06 } },
};

const item = {
  hidden: { opacity: 0, y: 16 },
  show: {
    opacity: 1,
    y: 0,
    transition: { type: 'spring' as const, stiffness: 260, damping: 26 },
  },
};

export function DashboardView() {
  const topology = useClusterStore((s) => s.topology);
  const latestMetrics = useClusterStore((s) => s.latestMetrics);
  const history = useClusterStore((s) => s.generationHistory);
  const apiOnline = useClusterStore((s) => s.apiOnline);
  const workerHealth = useClusterStore((s) => s.workerHealth);
  const masterRamPct = useClusterStore((s) => s.masterRamUtilizationPct);

  const workerNodes = topology?.nodes.filter((n) => n.role !== 'master') ?? [];
  const totalGb = topology ? topology.total_pooled_ram_mb / 1024 : 0;
  const usedGb = topology ? topology.used_pooled_ram_mb / 1024 : 0;
  const tokS = latestMetrics?.tokens_per_second ?? 0;
  // The poll publishes the master's own RAM every cycle; the generation
  // metrics carry the same reading. Unknown stays unknown — never "0.0%".
  const ramPct = masterRamPct ?? latestMetrics?.master_ram_utilization_pct ?? null;
  const mdnsOffline = topology?.discovery?.mdns === 'unavailable';

  if (!apiOnline && !topology) {
    return (
      <div className="glass-panel rounded-2xl p-8 text-center">
        <StatusDot tone="error" className="mx-auto mb-3 !h-3 !w-3" />
        <h2 className="text-lg font-semibold text-slate-100">API offline</h2>
        <p className="mt-1 text-sm text-slate-400">
          Cannot reach the local orchestrator API at 127.0.0.1:8080. Start the desktop
          app to see live cluster telemetry.
        </p>
      </div>
    );
  }

  return (
    <motion.div
      variants={container}
      initial="hidden"
      animate="show"
      className="space-y-5"
    >
      <motion.div variants={item} className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <GlowCard className="p-5" hover={false}>
          <div className="mb-2 flex items-center justify-between text-slate-400">
            <span className="text-[11px] font-semibold uppercase tracking-wider">
              Pooled memory
            </span>
            <Cpu className="h-4 w-4 text-emerald-400" />
          </div>
          <div className="text-3xl font-semibold tracking-tight text-slate-50">
            <NumberTicker value={totalGb} decimals={1} />
            <span className="ml-1 text-base font-normal text-slate-400">GB</span>
          </div>
          <div className="mt-1.5 flex items-center justify-between text-xs text-slate-400">
            <span>
              {usedGb.toFixed(1)} GB in use
            </span>
            <span className="font-mono text-emerald-400">
              {topology ? topology.nodes.length : 0} nodes
            </span>
          </div>
        </GlowCard>

        <GlowCard className="p-5" hover={false}>
          <div className="mb-2 flex items-center justify-between text-slate-400">
            <span className="text-[11px] font-semibold uppercase tracking-wider">
              Active model
            </span>
            <HardDrive className="h-4 w-4 text-cyan-400" />
          </div>
          <div className="truncate text-lg font-semibold text-slate-50">
            {topology?.active_model ?? 'None loaded'}
          </div>
          <div className="mt-1.5 text-xs text-slate-400">
            {topology?.active_model
              ? `Partitioned across ${topology.nodes.length} devices`
              : 'Pick a model in the catalog to begin'}
          </div>
        </GlowCard>

        <GlowCard className="p-5" hover={false}>
          <div className="mb-2 flex items-center justify-between text-slate-400">
            <span className="text-[11px] font-semibold uppercase tracking-wider">
              Throughput
            </span>
            <Zap className="h-4 w-4 text-amber-400" />
          </div>
          <div className="text-3xl font-semibold tracking-tight text-slate-50">
            <NumberTicker value={tokS} decimals={1} />
            <span className="ml-1 text-base font-normal text-slate-400">tok/s</span>
          </div>
          <Sparkline data={history} className="mt-2 h-8 w-full" />
        </GlowCard>

        <GlowCard className="p-5" hover={false}>
          <div className="mb-2 flex items-center justify-between text-slate-400">
            <span className="text-[11px] font-semibold uppercase tracking-wider">
              SSD mmap tier
            </span>
            <Activity className="h-4 w-4 text-emerald-400" />
          </div>
          <div
            className={`text-2xl font-semibold tracking-tight ${
              topology?.storage_mmap.enabled ? 'text-emerald-400' : 'text-slate-500'
            }`}
          >
            {topology?.storage_mmap.enabled ? 'READY' : 'DISABLED'}
          </div>
          <div className="mt-1.5 text-xs text-slate-400">
            {topology?.storage_mmap.enabled
              ? `${topology.storage_mmap.mapped_layers_count} layers memory-mapped`
              : 'Pager lands in Phase 4 — weights stream from RAM'}
          </div>
        </GlowCard>
      </motion.div>

      <motion.div variants={item}>
        <GlowCard className="p-5" hover={false}>
          <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-2">
              <h2 className="text-sm font-semibold uppercase tracking-wider text-slate-300">
                Live generation throughput
              </h2>
              <span className="flex items-center gap-1.5 rounded-full border border-emerald-400/30 bg-emerald-500/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wider text-emerald-300">
                <StatusDot />
                live
              </span>
            </div>
            <div className="font-mono text-xs text-slate-400">
              master RAM util{' '}
              <span className="text-emerald-300">
                {ramPct != null ? `${ramPct.toFixed(1)}%` : '—'}
              </span>
            </div>
          </div>
          <Sparkline
            data={history}
            className="h-28 w-full"
            stroke="#34D399"
            fill="rgba(52,211,153,0.12)"
          />
          {history.length === 0 && (
            <p className="-mt-6 text-center text-xs text-slate-500">
              Waiting for generation metrics… start a chat to see tokens flow.
            </p>
          )}
        </GlowCard>
      </motion.div>

      <motion.div variants={item}>
        <div className="mb-3 flex items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <h2 className="text-sm font-semibold uppercase tracking-wider text-slate-300">
              Cluster topology
            </h2>
            {mdnsOffline && (
              <span className="flex items-center gap-1.5 rounded-full border border-amber-400/30 bg-amber-500/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wider text-amber-300">
                mDNS unavailable
              </span>
            )}
          </div>
          <span className="text-xs text-slate-500">
            {workerNodes.length} worker{workerNodes.length === 1 ? '' : 's'} connected
          </span>
        </div>

        {topology && topology.nodes.length > 0 ? (
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
            {topology.nodes.map((node, index) => (
              <NodeCard
                key={node.node_id}
                node={node}
                index={index}
                health={workerHealth[node.node_id]}
              />
            ))}
          </div>
        ) : (
          <div className="glass-panel rounded-2xl p-8 text-center">
            {mdnsOffline ? (
              <p className="text-sm text-slate-400">
                mDNS discovery could not start — Wi-Fi worker discovery is offline.
                USB/ADB workers can still connect. Check that UDP multicast is
                allowed on this machine.
              </p>
            ) : (
              <p className="text-sm text-slate-400">
                Waiting for{' '}
                <code className="rounded bg-white/10 px-1.5 py-0.5 font-mono text-[12px] text-emerald-300">
                  _aiupcycler-worker._tcp.local.
                </code>{' '}
                discoveries…
              </p>
            )}
          </div>
        )}

        {workerNodes.length === 0 && topology && topology.nodes.length > 0 && (
          <p className="mt-3 text-xs text-slate-500">
            Only the host is visible — connect an Android worker over USB or Wi-Fi to
            pool its memory.
          </p>
        )}
      </motion.div>

      <motion.div variants={item}>
        <PairingPanel />
      </motion.div>
    </motion.div>
  );
}
