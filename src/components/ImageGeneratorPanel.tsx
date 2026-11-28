import { useEffect, useRef, useState } from 'react';
import { generateImage } from '../lib/api';
import type { ImageMetadata, ImageProvider, ImageSize } from '../lib/types';

interface ImageGeneratorPanelProps {
  onGenerated: (image: ImageMetadata) => void;
}

const PROVIDERS: { value: ImageProvider; label: string }[] = [
  { value: 'openai', label: 'OpenAI' },
  { value: 'gemini', label: 'Gemini' },
  { value: 'local', label: 'Local (unavailable)' },
];

const SIZES: { value: ImageSize; label: string }[] = [
  { value: 'square1024', label: 'Square 1024' },
  { value: 'portrait1024x1536', label: 'Portrait 1024x1536' },
  { value: 'landscape1536x1024', label: 'Landscape 1536x1024' },
];

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : 'Image generation failed';
}

export function ImageGeneratorPanel({ onGenerated }: ImageGeneratorPanelProps) {
  const [prompt, setPrompt] = useState('');
  const [provider, setProvider] = useState<ImageProvider>('openai');
  const [size, setSize] = useState<ImageSize>('square1024');
  const [busy, setBusy] = useState(false);
  const [errorSummary, setErrorSummary] = useState('');
  const [promptError, setPromptError] = useState(false);
  const errorRef = useRef<HTMLDivElement | null>(null);

  // The summary mounts with the error, so focus it after that render as well.
  useEffect(() => {
    if (errorSummary) errorRef.current?.focus();
  }, [errorSummary]);

  const submit = async () => {
    const trimmed = prompt.trim();
    if (trimmed.length === 0) {
      setPromptError(true);
      setErrorSummary('Prompt is required');
      errorRef.current?.focus();
      return;
    }

    setBusy(true);
    setErrorSummary('');
    setPromptError(false);
    try {
      const image = await generateImage({ prompt: trimmed, provider, size });
      onGenerated(image);
    } catch (reason) {
      setErrorSummary(errorMessage(reason));
      errorRef.current?.focus();
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      aria-labelledby="image-generator-heading"
      className="flex flex-col gap-4"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <h2 id="image-generator-heading" className="text-lg font-semibold text-slate-100">
        Generate an image
      </h2>

      {errorSummary && (
        <div
          ref={errorRef}
          role="alert"
          tabIndex={-1}
          className="image-error-summary text-sm"
        >
          {errorSummary}
        </div>
      )}

      <div className="flex flex-col gap-1.5">
        <label htmlFor="image-prompt" className="text-sm font-medium text-slate-200">
          Prompt
        </label>
        <textarea
          id="image-prompt"
          maxLength={4000}
          aria-describedby="image-prompt-help image-prompt-error"
          value={prompt}
          onChange={(event) => {
            setPrompt(event.target.value);
            if (promptError && event.target.value.trim().length > 0) setPromptError(false);
          }}
          rows={4}
          className="w-full rounded-lg border border-[var(--image-border)] bg-[var(--image-bg)] p-2.5 text-sm text-slate-100 placeholder:text-slate-400"
          placeholder="Describe the image you want"
        />
        <p id="image-prompt-help" className="text-xs text-slate-300">
          Describe the subject, style, and composition. Up to 4000 characters.
        </p>
        {promptError && (
          <p id="image-prompt-error" className="text-sm font-medium text-[#FCA5A5]">
            Prompt is required
          </p>
        )}
      </div>

      <div className="flex flex-col gap-1.5">
        <label htmlFor="image-provider" className="text-sm font-medium text-slate-200">
          Provider
        </label>
        <select
          id="image-provider"
          value={provider}
          onChange={(event) => setProvider(event.target.value as ImageProvider)}
          className="min-h-11 w-full rounded-lg border border-[var(--image-border)] bg-[var(--image-bg)] px-2.5 text-sm text-slate-100"
        >
          {PROVIDERS.map((entry) => (
            <option key={entry.value} value={entry.value}>
              {entry.label}
            </option>
          ))}
        </select>
      </div>

      <div className="flex flex-col gap-1.5">
        <label htmlFor="image-size" className="text-sm font-medium text-slate-200">
          Size
        </label>
        <select
          id="image-size"
          value={size}
          onChange={(event) => setSize(event.target.value as ImageSize)}
          className="min-h-11 w-full rounded-lg border border-[var(--image-border)] bg-[var(--image-bg)] px-2.5 text-sm text-slate-100"
        >
          {SIZES.map((entry) => (
            <option key={entry.value} value={entry.value}>
              {entry.label}
            </option>
          ))}
        </select>
      </div>

      <button
        type="submit"
        disabled={busy || prompt.trim().length === 0}
        className="min-h-11 rounded-lg border border-[var(--image-accent)] bg-[var(--image-accent)] px-4 text-sm font-semibold text-slate-950 transition-opacity duration-150 disabled:cursor-not-allowed disabled:opacity-50"
      >
        Generate image
      </button>
    </form>
  );
}
