import { motion } from 'motion/react';
import { ProgressBar } from './ui/ProgressBar';
import { StatusDot } from './ui/StatusDot';
import type { TopologyNode } from '../lib/types';

function formatMb(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${Math.round(mb)} MB`;
}

function connectionTone(connection: string): 'ok' | 'warn' | 'idle' {
  const value = connection.toUpperCase();
  if (value.includes('USB')) return 'ok';
  if (value.includes('WIFI') || value.includes('MDNS')) return 'warn';
  return 'idle';
}

interface NodeCardProps {
  node: TopologyNode;
  index: number;
}

export function NodeCard({ node, index }: NodeCardProps) {
  const ramRatio = node.ram_total_mb > 0 ? node.ram_assigned_mb / node.ram_total_mb : 0;
  const isMaster = node.role === 'master';

  return (
    <motion.div
      initial={{ opacity: 0, y: 18, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={{
        type: 'spring',
        stiffness: 260,
        damping: 26,
        delay: Math.min(index * 0.06, 0.4),
      }}
      className={`glass-panel glow-border group relative rounded-2xl p-4 transition-shadow duration-200 hover:shadow-[0_0_28px_rgba(16,185,129,0.12)] ${
        isMaster ? 'ring-1 ring-emerald-400/30' : ''
      }`}
    >
      <div className="mb-3 flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <StatusDot tone={connectionTone(node.connection)} />
            <span className="truncate text-sm font-semibold text-slate-100">
              {node.device_name}
            </span>
          </div>
          <div className="mt-1 flex items-center gap-2 text-[11px] text-slate-400">
            <span className="rounded-md border border-white/10 bg-white/5 px-1.5 py-0.5 font-mono uppercase tracking-wide">
              {node.connection}
            </span>
            <span
              className={`rounded-md px-1.5 py-0.5 font-medium ${
                isMaster
                  ? 'bg-emerald-500/15 text-emerald-300'
                  : 'bg-cyan-500/15 text-cyan-300'
              }`}
            >
              {node.role}
            </span>
          </div>
        </div>
        <div className="text-right">
          <div className="font-mono text-xs text-slate-300">
            {node.ping_ms < 1 ? '<1' : node.ping_ms.toFixed(1)} ms
          </div>
          <div className="text-[10px] uppercase tracking-wider text-slate-500">ping</div>
        </div>
      </div>

      <div className="mb-1.5 flex items-baseline justify-between text-xs">
        <span className="text-slate-400">RAM assigned</span>
        <span className="font-mono text-slate-200">
          {formatMb(node.ram_assigned_mb)}
          <span className="text-slate-500"> / {formatMb(node.ram_total_mb)}</span>
        </span>
      </div>
      <ProgressBar
        value={ramRatio}
        ariaLabel={`RAM usage for ${node.device_name}`}
        shimmer={ramRatio > 0}
      />

      <div className="mt-3 flex flex-wrap gap-1.5">
        {node.assigned_layers.length === 0 ? (
          <span className="text-[11px] text-slate-500">no layers assigned</span>
        ) : (
          node.assigned_layers.map(([start, end]) => (
            <span
              key={`${start}-${end}`}
              className="rounded-md border border-emerald-400/25 bg-emerald-500/10 px-2 py-0.5 font-mono text-[11px] text-emerald-300"
            >
              L{start} → {end}
            </span>
          ))
        )}
      </div>
    </motion.div>
  );
}
