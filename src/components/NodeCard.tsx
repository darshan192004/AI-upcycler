import { motion } from 'motion/react';
import { Battery, Thermometer } from 'lucide-react';
import { ProgressBar } from './ui/ProgressBar';
import { StatusDot } from './ui/StatusDot';
import type { TopologyNode, TopologyThermal, WorkerHealth } from '../lib/types';

/**
 * A snapshot older than this is shown dimmed: the value was really measured,
 * but the worker has stopped answering and it is no longer current.
 *
 * Three poll intervals (the master polls every 5 s).
 */
const STALE_AFTER_MS = 15_000;

function formatMb(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${Math.round(mb)} MB`;
}

function formatOptionalMb(mb: number | null): string {
  return mb == null ? '—' : formatMb(mb);
}

function stateLabel(state: string): string {
  switch (state) {
    case 'usb_detected':
      return 'USB detected';
    case 'discovered':
      return 'discovered';
    case 'paired':
      return 'paired';
    case 'ready':
      return 'ready';
    case 'master':
      return 'master';
    default:
      return state;
  }
}

function stateTone(state: string): string {
  switch (state) {
    case 'ready':
    case 'master':
      return 'border-emerald-400/25 bg-emerald-500/10 text-emerald-300';
    case 'usb_detected':
      return 'border-amber-400/25 bg-amber-500/10 text-amber-300';
    case 'paired':
      return 'border-cyan-400/25 bg-cyan-500/10 text-cyan-300';
    default:
      return 'border-white/10 bg-white/5 text-slate-400';
  }
}

function stateTitle(state: string): string {
  switch (state) {
    case 'usb_detected':
      return 'ADB can see the phone, but the AI Upcycler worker app has not answered yet.';
    case 'discovered':
      return 'Discovered on Wi-Fi, but not paired with this master yet.';
    case 'paired':
      return 'Paired, but no model engine is open, so it cannot receive layers.';
    case 'ready':
      return 'Paired and serving an open model engine.';
    case 'master':
      return 'This machine coordinates the cluster and holds no layers.';
    default:
      return state;
  }
}

function stateDotTone(state: string): 'ok' | 'warn' | 'idle' {
  if (state === 'ready' || state === 'master') return 'ok';
  if (state === 'paired' || state === 'usb_detected') return 'warn';
  return 'idle';
}

interface NodeCardProps {
  node: TopologyNode;
  index: number;
  /** The worker's latest measured health, when the master has one. */
  health?: WorkerHealth;
  /** The thermal state the layer plan was computed against. */
  thermal?: TopologyThermal;
}

export function NodeCard({ node, index, health, thermal }: NodeCardProps) {
  const ramRatio =
    node.ram_used_mb != null && node.ram_total_mb > 0
      ? Math.min(1, node.ram_used_mb / node.ram_total_mb)
      : 0;
  const isMaster = node.role === 'master';
  const pingKnown = isMaster || node.worker_state === 'ready' || node.ping_ms > 0;
  const stale =
    health !== undefined && Date.now() - health.observed_at_epoch_ms > STALE_AFTER_MS;
  const measured = health !== undefined && !stale;

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
            <StatusDot tone={stateDotTone(node.worker_state)} />
            <span className="truncate text-sm font-semibold text-slate-100">
              {node.device_name}
            </span>
          </div>
          <div className="mt-1 flex flex-wrap items-center gap-2 text-[11px] text-slate-400">
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
            {!isMaster && (
              <span
                className={`rounded-md border px-1.5 py-0.5 font-medium ${stateTone(
                  node.worker_state,
                )}`}
                title={stateTitle(node.worker_state)}
              >
                {stateLabel(node.worker_state)}
              </span>
            )}
          </div>
        </div>
        <div className="text-right">
          <div className="font-mono text-xs text-slate-300">
            {pingKnown ? `${node.ping_ms < 1 ? '<1' : node.ping_ms.toFixed(1)} ms` : '—'}
          </div>
          <div className="text-[10px] uppercase tracking-wider text-slate-500">ping</div>
        </div>
      </div>

      <div className="mb-1.5 flex items-baseline justify-between text-xs">
        <span className="text-slate-400">RAM in use</span>
        <span className={`font-mono ${stale ? 'text-slate-500' : 'text-slate-200'}`}>
          {formatOptionalMb(node.ram_used_mb)}
          <span className="text-slate-500"> / {formatMb(node.ram_total_mb)}</span>
        </span>
      </div>
      <ProgressBar
        value={ramRatio}
        ariaLabel={`RAM in use on ${node.device_name}`}
        shimmer={ramRatio > 0}
      />
      <div className="mt-1 flex items-center justify-between text-[11px] text-slate-500">
        <span>
          {node.ram_available_mb != null
            ? `${formatMb(node.ram_available_mb)} available`
            : 'available —'}
        </span>
        <span title="Scheduler assignment; the master holds no layers.">
          assigned {formatMb(node.ram_assigned_mb)}
        </span>
      </div>

      {!isMaster && (
        <div className="mt-3 flex flex-wrap items-center gap-1.5">
          <span
            className={`flex items-center gap-1 rounded-md border px-2 py-0.5 font-mono text-[11px] ${
              measured && health?.temp_celsius != null
                ? 'border-cyan-400/25 bg-cyan-500/10 text-cyan-300'
                : 'border-white/10 bg-white/5 text-slate-500'
            }`}
            title={
              health
                ? `Measured at ${new Date(health.observed_at_epoch_ms).toLocaleTimeString()}`
                : 'Not measured yet'
            }
          >
            <Thermometer className="h-3 w-3" aria-hidden="true" />
            {health?.temp_celsius != null ? `${health.temp_celsius.toFixed(1)} °C` : '—'}
          </span>
          <span
            className={`flex items-center gap-1 rounded-md border px-2 py-0.5 font-mono text-[11px] ${
              measured && health?.battery_pct != null
                ? 'border-emerald-400/25 bg-emerald-500/10 text-emerald-300'
                : 'border-white/10 bg-white/5 text-slate-500'
            }`}
            title={
              health
                ? `Measured at ${new Date(health.observed_at_epoch_ms).toLocaleTimeString()}`
                : 'Not measured yet'
            }
          >
            <Battery className="h-3 w-3" aria-hidden="true" />
            {health?.battery_pct != null ? `${Math.round(health.battery_pct)}%` : '—'}
          </span>
          {stale && (
            <span className="text-[10px] uppercase tracking-wider text-slate-600">stale</span>
          )}
        </div>
      )}

      {thermal?.capped && (
        <div className="mt-2 flex items-center gap-1.5 text-[11px] text-amber-300">
          <Thermometer className="h-3 w-3" aria-hidden="true" />
          <span>
            resting — {thermal.temp_celsius.toFixed(1)} °C, so it takes a smaller share of the
            model
          </span>
        </div>
      )}

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
