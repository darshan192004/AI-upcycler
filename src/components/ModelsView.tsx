import { useEffect, useState } from 'react';
import { motion } from 'motion/react';
import { CheckCircle2, Download, HardDrive, Loader2, PackageOpen } from 'lucide-react';
import { startDownload } from '../lib/api';
import { HF_CATALOG, type CatalogEntry } from '../lib/catalog';
import { useClusterStore } from '../store/cluster';
import { GlowCard } from './ui/GlowCard';
import { ProgressBar } from './ui/ProgressBar';
import { StatusDot } from './ui/StatusDot';

function formatBytes(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(0)} MB`;
  return `${bytes} B`;
}

export function ModelsView() {
  const models = useClusterStore((s) => s.models);
  const refreshModels = useClusterStore((s) => s.refreshModels);
  const downloads = useClusterStore((s) => s.downloads);
  const apiOnline = useClusterStore((s) => s.apiOnline);
  const [error, setError] = useState<string | null>(null);
  const [customRepo, setCustomRepo] = useState('');
  const [customFile, setCustomFile] = useState('');

  useEffect(() => {
    if (models.length === 0) void refreshModels();
  }, [models.length, refreshModels]);

  async function download(repoId: string, fileName: string) {
    setError(null);
    try {
      await startDownload({ repo_id: repoId, file_name: fileName });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  const installedIds = new Set(models.map((m) => m.id));

  return (
    <div className="space-y-6">
      {error && (
        <div className="rounded-lg border border-red-400/30 bg-red-500/10 px-4 py-2.5 text-xs text-red-300">
          {error}
        </div>
      )}

      <section>
        <div className="mb-3 flex items-center justify-between">
          <h2 className="flex items-center gap-2 text-sm font-semibold uppercase tracking-wider text-slate-300">
            <HardDrive className="h-4 w-4 text-emerald-400" />
            Local models
          </h2>
          <span className="text-xs text-slate-500">{models.length} available</span>
        </div>

        {models.length === 0 ? (
          <div className="glass-panel flex flex-col items-center rounded-2xl p-10 text-center">
            <PackageOpen className="mb-3 h-8 w-8 text-slate-500" />
            <p className="text-sm text-slate-400">
              {apiOnline
                ? 'No models yet — download one from the catalog below.'
                : 'API offline — cannot list local models.'}
            </p>
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
            {models.map((model, index) => (
              <motion.div
                key={model.id}
                initial={{ opacity: 0, y: 14 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ delay: Math.min(index * 0.05, 0.3) }}
              >
                <GlowCard className="p-5" hover={false}>
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <StatusDot tone="ok" pulse={false} />
                        <span className="truncate text-sm font-semibold text-slate-100">
                          {model.id}
                        </span>
                      </div>
                      <span className="mt-1.5 inline-block rounded-md border border-cyan-400/25 bg-cyan-500/10 px-2 py-0.5 font-mono text-[11px] text-cyan-300">
                        {model.meta.quantization}
                      </span>
                    </div>
                    <span className="rounded-md bg-emerald-500/12 px-2 py-1 text-[10px] font-semibold uppercase tracking-wider text-emerald-300">
                      ready
                    </span>
                  </div>
                  <dl className="mt-4 grid grid-cols-3 gap-2 border-t border-white/8 pt-3 text-center">
                    <div>
                      <dt className="text-[10px] uppercase tracking-wider text-slate-500">
                        size
                      </dt>
                      <dd className="font-mono text-xs text-slate-200">
                        {formatBytes(model.meta.file_size_bytes)}
                      </dd>
                    </div>
                    <div>
                      <dt className="text-[10px] uppercase tracking-wider text-slate-500">
                        layers
                      </dt>
                      <dd className="font-mono text-xs text-slate-200">
                        {model.meta.layers}
                      </dd>
                    </div>
                    <div>
                      <dt className="text-[10px] uppercase tracking-wider text-slate-500">
                        min ram
                      </dt>
                      <dd className="font-mono text-xs text-slate-200">
                        {model.meta.recommended_min_ram_gb} GB
                      </dd>
                    </div>
                  </dl>
                </GlowCard>
              </motion.div>
            ))}
          </div>
        )}
      </section>

      <section>
        <div className="mb-3 flex items-center justify-between">
          <h2 className="flex items-center gap-2 text-sm font-semibold uppercase tracking-wider text-slate-300">
            <Download className="h-4 w-4 text-cyan-400" />
            Hugging Face catalog
          </h2>
          <span className="text-xs text-slate-500">curated GGUF builds</span>
        </div>

        <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
          {HF_CATALOG.map((entry: CatalogEntry, index) => {
            const progress = downloads[entry.file_name];
            const downloading = progress?.status === 'running';
            const complete =
              progress?.status === 'complete' ||
              installedIds.has(entry.file_name.replace(/\.gguf$/, ''));
            const pct =
              progress && progress.total_bytes > 0
                ? progress.received_bytes / progress.total_bytes
                : 0;

            return (
              <motion.div
                key={entry.file_name}
                initial={{ opacity: 0, y: 14 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ delay: Math.min(index * 0.05, 0.3) }}
              >
                <GlowCard className="flex h-full flex-col p-5" hover={!downloading}>
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0">
                      <span className="block truncate text-sm font-semibold text-slate-100">
                        {entry.display_name}
                      </span>
                      <span className="mt-0.5 block truncate text-[11px] text-slate-500">
                        {entry.repo_id}
                      </span>
                    </div>
                    <span className="shrink-0 rounded-md border border-white/10 bg-white/5 px-2 py-1 font-mono text-[11px] text-slate-300">
                      {entry.parameter_size} · {entry.quantization}
                    </span>
                  </div>

                  <div className="mt-3 flex flex-1 flex-wrap gap-1.5 text-[11px] text-slate-400">
                    <span className="rounded-md bg-white/5 px-2 py-0.5">
                      {entry.file_size_gb} GB
                    </span>
                    <span className="rounded-md bg-white/5 px-2 py-0.5">
                      needs {entry.min_ram_gb} GB
                    </span>
                    <span className="rounded-md bg-white/5 px-2 py-0.5">
                      {entry.layers} layers
                    </span>
                  </div>

                  <div className="mt-4">
                    {downloading || progress ? (
                      <div>
                        <div className="mb-1.5 flex items-center justify-between text-[11px]">
                          <span className="flex items-center gap-1.5 text-emerald-300">
                            {downloading ? (
                              <Loader2 className="h-3 w-3 animate-spin" />
                            ) : (
                              <CheckCircle2 className="h-3 w-3" />
                            )}
                            {downloading
                              ? `Downloading ${Math.round(pct * 100)}%`
                              : progress?.status === 'complete'
                                ? 'Download complete'
                                : 'Download failed'}
                          </span>
                          <span className="font-mono text-slate-500">
                            {formatBytes(progress?.received_bytes ?? 0)}
                          </span>
                        </div>
                        <ProgressBar
                          value={pct}
                          ariaLabel={`Download progress for ${entry.display_name}`}
                          shimmer={downloading}
                          barClassName={
                            progress?.status === 'error'
                              ? 'bg-red-400'
                              : 'bg-gradient-to-r from-emerald-500 to-cyan-400'
                          }
                        />
                      </div>
                    ) : (
                      <button
                        onClick={() => void download(entry.repo_id, entry.file_name)}
                        disabled={!apiOnline || complete}
                        className="flex w-full cursor-pointer items-center justify-center gap-2 rounded-xl border border-emerald-400/30 bg-emerald-500/10 py-2.5 text-xs font-semibold text-emerald-300 transition-colors hover:border-emerald-400/60 hover:bg-emerald-500/20 disabled:cursor-not-allowed disabled:border-white/8 disabled:bg-white/5 disabled:text-slate-500"
                      >
                        {complete ? (
                          <>
                            <CheckCircle2 className="h-3.5 w-3.5" /> Installed
                          </>
                        ) : (
                          <>
                            <Download className="h-3.5 w-3.5" /> Download{' '}
                            {entry.file_size_gb} GB
                          </>
                        )}
                      </button>
                    )}
                  </div>
                </GlowCard>
              </motion.div>
            );
          })}
        </div>
      </section>

      <section className="glass-panel rounded-2xl p-5">
        <h2 className="mb-3 text-sm font-semibold uppercase tracking-wider text-slate-300">
          Custom repository
        </h2>
        <div className="flex flex-col gap-3 md:flex-row">
          <input
            value={customRepo}
            onChange={(e) => setCustomRepo(e.target.value)}
            placeholder="org/repo (e.g. bartowski/Llama-3.2-1B-Instruct-GGUF)"
            className="flex-1 rounded-xl border border-white/10 bg-white/5 px-4 py-2.5 font-mono text-xs text-slate-100 placeholder:text-slate-500 outline-none transition-colors focus:border-cyan-400/50"
          />
          <input
            value={customFile}
            onChange={(e) => setCustomFile(e.target.value)}
            placeholder="file.gguf"
            className="md:w-64 rounded-xl border border-white/10 bg-white/5 px-4 py-2.5 font-mono text-xs text-slate-100 placeholder:text-slate-500 outline-none transition-colors focus:border-cyan-400/50"
          />
          <button
            onClick={() => void download(customRepo.trim(), customFile.trim())}
            disabled={!apiOnline || !customRepo.trim() || !customFile.trim()}
            className="cursor-pointer rounded-xl bg-gradient-to-br from-cyan-400 to-cyan-600 px-5 py-2.5 text-xs font-semibold text-slate-950 transition-all hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
          >
            Download
          </button>
        </div>
      </section>
    </div>
  );
}
