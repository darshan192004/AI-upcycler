import { useEffect, useState } from 'react';
import { getSettingsRecommendations, putSettings } from '../lib/api';
import type { AppSettings, SettingsRecommendation } from '../lib/types';

interface SettingsPanelProps {
  settings: AppSettings | null;
  onSettingsChange: (settings: AppSettings) => void;
}

export function SettingsPanel({ settings, onSettingsChange }: SettingsPanelProps) {
  const [recommendation, setRecommendation] = useState<SettingsRecommendation | null>(null);
  const [draft, setDraft] = useState<AppSettings | null>(settings);
  const [error, setError] = useState<string | null>(null);
  const [writing, setWriting] = useState(false);

  useEffect(() => {
    void getSettingsRecommendations()
      .then(setRecommendation)
      .catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)));
  }, []);

  useEffect(() => setDraft(settings), [settings]);

  if (!draft) return null;
  // Bind the narrowed value: closures do not keep the guard's narrowing.
  const current: AppSettings = draft;

  async function save() {
    if (writing) return;
    setWriting(true);
    try {
      setError(null);
      const saved = await putSettings(current);
      // A successful empty-body write leaves the draft as the current value.
      onSettingsChange(saved ?? current);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setWriting(false);
    }
  }

  function applyRecommendation() {
    if (writing || !recommendation) return;
    const next: AppSettings = {
      ...current,
      context_tokens: recommendation.context_tokens,
      max_tokens: recommendation.max_tokens,
    };
    setDraft(next);
    setWriting(true);
    void putSettings(next)
      .then((saved) => onSettingsChange(saved ?? next))
      .catch((err: unknown) =>
        setError(err instanceof Error ? err.message : String(err)),
      )
      .finally(() => setWriting(false));
  }

  return (
    <section aria-label="Generation settings" className="rounded-xl border border-[#475569] bg-[#1B2336] p-3 text-[#94A3B8]">
      <h3 className="mb-2 text-xs font-semibold text-slate-100">Generation settings</h3>
      {recommendation && (
        <button
          type="button"
          onClick={applyRecommendation}
          disabled={writing}
          className="min-h-11 rounded-lg border border-[#22C55E] px-3 text-xs text-[#22C55E] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E] disabled:cursor-not-allowed disabled:opacity-40"
          aria-label="Apply recommended context and output limits"
        >
          Apply recommended: {recommendation.context_tokens}/{recommendation.max_tokens}
        </button>
      )}
      <label className="mt-3 block text-xs">
        Context tokens
        <input
          aria-label="Context tokens"
          type="number"
          disabled={writing}
          min={1024}
          max={65536}
          value={draft.context_tokens}
          onChange={(event) =>
            setDraft({ ...draft, context_tokens: Number(event.target.value) })
          }
          className="mt-1 min-h-11 w-full rounded-lg border border-[#475569] bg-[#0F172A] px-2 py-1 text-[#E2E8F0] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E]"
        />
      </label>
      <label className="mt-2 block text-xs">
        Max tokens
        <input
          aria-label="Max tokens"
          type="number"
          disabled={writing}
          min={1}
          max={32768}
          value={draft.max_tokens}
          onChange={(event) => setDraft({ ...draft, max_tokens: Number(event.target.value) })}
          className="mt-1 min-h-11 w-full rounded-lg border border-[#475569] bg-[#0F172A] px-2 py-1 text-[#E2E8F0] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E]"
        />
      </label>
      <button
        type="button"
        onClick={() => void save()}
        disabled={writing}
        className="mt-3 min-h-11 rounded-lg bg-[#22C55E] px-3 text-xs font-semibold text-[#0F172A] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E] disabled:cursor-not-allowed disabled:opacity-40"
        aria-label="Save generation settings"
      >
        Save settings
      </button>
      {error && <p className="mt-2 text-xs text-[#EF4444]">{error}</p>}
    </section>
  );
}
