import { useCallback, useEffect, useState } from 'react';
import { Radar, RefreshCw, Trash2, Upload } from 'lucide-react';
import {
  connectDevice,
  getAvailableDevices,
  getProvisionState,
  installWorker,
  revokeDevice,
  scanNetwork,
  startProvision,
} from '../lib/api';
import type {
  AvailableDevice,
  DeviceTransport,
  OnboardingState,
  ProvisionState,
} from '../lib/types';
import { useClusterStore } from '../store/cluster';
import { GlowCard } from './ui/GlowCard';

/**
 * Phones that can be reached but are not in the cluster yet (spec 2026-11-03,
 * slice A).
 *
 * The section only ever lists devices we can actually act on. A phone with no
 * ADB reachability and no worker app is not shown, because every button we
 * could draw for it would fail.
 */
const TRANSPORT_LABEL: Record<DeviceTransport, string> = {
  usb: 'USB',
  wireless_debugging: 'Wireless debugging',
  wifi_adb: 'Wi-Fi',
};

type Action = 'connect' | 'install' | 'revoke' | null;

/** The same units the Models page uses, so sizes read consistently. */
function formatBytes(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(0)} MB`;
  return `${bytes} B`;
}

function describe(state: OnboardingState): { label: string; action: Action; tone: string } {
  switch (state.state) {
    case 'discovered':
      return { label: 'Not connected', action: 'connect', tone: 'text-slate-400' };
    case 'connecting':
      return { label: 'Connecting…', action: null, tone: 'text-cyan-300' };
    case 'awaiting_consent':
      return {
        label: 'Waiting for Allow on the phone…',
        action: null,
        tone: 'text-amber-300',
      };
    case 'connected':
      return { label: 'Connected', action: null, tone: 'text-cyan-300' };
    case 'needs_install':
      return {
        label:
          state.installed == null
            ? 'Worker app not installed'
            : `Worker app ${state.installed} is older than ${state.available}`,
        action: 'install',
        tone: 'text-amber-300',
      };
    case 'installing':
      return { label: 'Installing…', action: null, tone: 'text-cyan-300' };
    case 'needs_enrol':
      return {
        label: 'App ready — enrol it to join the cluster',
        action: null,
        tone: 'text-amber-300',
      };
    case 'ready':
      return { label: 'In the cluster', action: 'revoke', tone: 'text-emerald-300' };
    case 'failed':
      return { label: state.reason, action: 'connect', tone: 'text-rose-300' };
  }
}

const buttonClass =
  'rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs font-medium text-slate-100 transition-colors hover:border-cyan-400/40 hover:bg-white/10 disabled:cursor-not-allowed disabled:opacity-40';

export function AvailableDevices() {
  const [devices, setDevices] = useState<AvailableDevice[]>([]);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [subnet, setSubnet] = useState('');
  const [scanning, setScanning] = useState(false);
  /** The push each device is in, keyed by device id (spec D-P8). */
  const [provision, setProvision] = useState<Record<string, ProvisionState>>({});

  // What a push would deliver: the library head the topology reports as
  // active, so the card's size and name match what the cluster serves.
  const models = useClusterStore((s) => s.models);
  const activeModelId = useClusterStore((s) => s.topology?.active_model ?? null);
  const activeModel = models.find((model) => model.id === activeModelId) ?? null;

  const refresh = useCallback(async () => {
    try {
      const response = await getAvailableDevices();
      setDevices(response.devices);
    } catch (e) {
      // A build without adb answers 503 with the reason; show it once rather
      // than replacing the list with a red box on every poll.
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), 4000);
    return () => clearInterval(timer);
  }, [refresh]);

  const refreshProvision = useCallback(async (id: string) => {
    try {
      const state = await getProvisionState(id);
      setProvision((prev) => ({ ...prev, [id]: state }));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  // While any push runs, ask for its state every 2 s (spec D-P8: the server
  // keeps progress in memory and the dashboard polls; no telemetry event).
  const hasRunningPush = Object.values(provision).some(
    (state) => state.state === 'running',
  );
  useEffect(() => {
    if (!hasRunningPush) return;
    const timer = window.setInterval(() => {
      for (const [id, state] of Object.entries(provision)) {
        if (state.state === 'running') void refreshProvision(id);
      }
    }, 2000);
    return () => window.clearInterval(timer);
  }, [hasRunningPush, provision, refreshProvision]);

  const run = async (device: AvailableDevice, action: Exclude<Action, null>) => {
    setBusyId(device.id);
    setError(null);
    try {
      if (action === 'connect') await connectDevice(device.id);
      if (action === 'install') await installWorker(device.id);
      if (action === 'revoke') {
        await revokeDevice(device.id);
        // The enrollment is gone, so any earlier push state describes a
        // device that no longer exists; drop it rather than show it again
        // if the phone pairs a second time.
        setProvision((prev) => {
          const next = { ...prev };
          delete next[device.id];
          return next;
        });
      }
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusyId(null);
    }
  };

  /** Starts a push; the reply is only the starting state (spec D-P5/D-P8). */
  const startPush = async (device: AvailableDevice) => {
    if (!activeModel) return;
    setBusyId(device.id);
    setError(null);
    try {
      const state = await startProvision(device.id, activeModel.id);
      setProvision((prev) => ({ ...prev, [device.id]: state }));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      // A 409 means some push already owns the device; read its state so the
      // card shows that transfer instead of only the refusal.
      void refreshProvision(device.id);
    } finally {
      setBusyId(null);
    }
  };

  const scan = async () => {
    const target = subnet.trim();
    if (!target) return;
    setScanning(true);
    setError(null);
    try {
      await scanNetwork(target);
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setScanning(false);
    }
  };

  return (
    <GlowCard>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <Radar className="h-4 w-4 text-cyan-300" aria-hidden />
          <h2 className="text-sm font-semibold text-slate-100">Available on network</h2>
        </div>
        <button
          type="button"
          className={buttonClass}
          onClick={() => void refresh()}
          disabled={busyId !== null}
        >
          <RefreshCw className="mr-1 inline h-3 w-3" aria-hidden />
          Refresh
        </button>
      </div>

      {devices.length === 0 ? (
        <p className="mt-3 text-xs text-slate-400">
          No phone is reachable over ADB right now. Plug one in, or enable Wireless
          debugging on Android 11+.
        </p>
      ) : (
        <ul className="mt-3 divide-y divide-white/5">
          {devices.map((device) => {
            const view = describe(device.state);
            const busy = busyId === device.id;
            const push = provision[device.id];
            const pct =
              push?.state === 'running' && push.total_bytes > 0
                ? Math.floor((push.bytes_done / push.total_bytes) * 100)
                : 0;
            // Hidden while a transfer is in flight or already delivered
            // (spec D-P8): there is nothing left to start.
            const canProvision =
              device.state.state === 'ready' &&
              activeModel !== null &&
              push?.state !== 'running' &&
              push?.state !== 'done';
            return (
              <li key={device.id} className="flex flex-wrap items-center gap-3 py-3">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-sm text-slate-100">
                      {device.display_name}
                    </span>
                    <span className="rounded-full border border-white/10 px-2 py-0.5 text-[10px] uppercase tracking-wide text-slate-400">
                      {TRANSPORT_LABEL[device.transport]}
                    </span>
                  </div>
                  <p className={`mt-1 text-xs ${view.tone}`}>{view.label}</p>
                  {device.state.state === 'failed' && (
                    <p className="mt-1 text-xs text-slate-400">{device.state.remedy}</p>
                  )}
                  {canProvision && activeModel && (
                    <p className="mt-1 text-[11px] text-slate-500">
                      Active model: {activeModel.id} ·{' '}
                      {formatBytes(activeModel.meta.file_size_bytes)}
                    </p>
                  )}
                  {push?.state === 'running' && (
                    <p className="mt-1 text-xs text-cyan-300" role="status">
                      Pushing {formatBytes(push.bytes_done)} /{' '}
                      {formatBytes(push.total_bytes)} ({pct}%)
                    </p>
                  )}
                  {push?.state === 'done' && (
                    <p className="mt-1 text-xs text-emerald-300">
                      Model installed ({formatBytes(push.bytes)}) — the worker
                      reopens it a moment later
                    </p>
                  )}
                  {push?.state === 'failed' && (
                    <p className="mt-1 text-xs text-rose-300">{push.message}</p>
                  )}
                </div>

                <div className="flex items-center gap-2">
                  {canProvision && activeModel && (
                    <button
                      type="button"
                      className={buttonClass}
                      disabled={busy || busyId !== null}
                      onClick={() => void startPush(device)}
                      title={`Push ${activeModel.id} to this phone over Wi-Fi`}
                    >
                      <Upload className="mr-1 inline h-3 w-3" aria-hidden />
                      Provision model ({formatBytes(activeModel.meta.file_size_bytes)})
                    </button>
                  )}

                  {view.action === 'revoke' ? (
                    <button
                      type="button"
                      className={buttonClass}
                      disabled={busy}
                      onClick={() => void run(device, 'revoke')}
                      title="Remove the enrollment and drop the connection"
                    >
                      <Trash2 className="mr-1 inline h-3 w-3" aria-hidden />
                      Revoke
                    </button>
                  ) : (
                    view.action !== null && (
                      <button
                        type="button"
                        className={buttonClass}
                        disabled={busy || busyId !== null}
                        onClick={() => void run(device, view.action as Exclude<Action, null>)}
                      >
                        {busy
                          ? 'Working…'
                          : device.state.state === 'failed'
                            ? 'Retry'
                            : device.state.state === 'needs_install'
                              ? 'Install'
                              : 'Connect'}
                      </button>
                    )
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      )}

      <div className="mt-4 flex flex-wrap items-center gap-2 border-t border-white/5 pt-3">
        <input
          className="rounded-xl border border-white/10 bg-white/5 px-3 py-1.5 font-mono text-xs text-slate-100 placeholder:text-slate-500 outline-none focus:border-cyan-400/50"
          placeholder="192.168.1"
          value={subnet}
          onChange={(e) => setSubnet(e.target.value)}
          aria-label="Subnet to scan"
        />
        <button
          type="button"
          className={buttonClass}
          onClick={() => void scan()}
          disabled={scanning || subnet.trim().length === 0}
        >
          {scanning ? 'Scanning…' : 'Scan network'}
        </button>
        <span className="text-[11px] text-slate-500">
          A scan probes port 5555 on every address in that range. Only do it on your own
          network.
        </span>
      </div>

      {error && <p className="mt-3 text-xs text-rose-300">{error}</p>}
    </GlowCard>
  );
}
