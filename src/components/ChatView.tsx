import { useEffect, useMemo, useRef, useState } from 'react';
import { AnimatePresence, motion, useReducedMotion } from 'motion/react';
import ReactMarkdown from 'react-markdown';
import { AlertTriangle, Send, Square } from 'lucide-react';
import { streamChat } from '../lib/api';
import type { ChatMessage } from '../lib/types';
import { useClusterStore } from '../store/cluster';

interface DisplayMessage extends ChatMessage {
  id: number;
}

let nextId = 1;

const SUGGESTIONS = [
  'Explain how layer partitioning works',
  'What is AI Upcycler?',
  'Write a haiku about distributed computing',
];

export function ChatView() {
  const models = useClusterStore((s) => s.models);
  const refreshModels = useClusterStore((s) => s.refreshModels);
  const apiOnline = useClusterStore((s) => s.apiOnline);

  const [selectedModel, setSelectedModel] = useState('');
  const [messages, setMessages] = useState<DisplayMessage[]>([]);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const bottomRef = useRef<HTMLDivElement | null>(null);
  const reduceMotion = useReducedMotion();

  useEffect(() => {
    if (models.length === 0) void refreshModels();
  }, [models.length, refreshModels]);

  useEffect(() => {
    if (!selectedModel && models.length > 0) setSelectedModel(models[0].id);
  }, [models, selectedModel]);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: reduceMotion ? 'auto' : 'smooth' });
  }, [messages, reduceMotion]);

  const canSend = useMemo(
    () => draft.trim().length > 0 && !busy && selectedModel.length > 0,
    [draft, busy, selectedModel],
  );

  async function send(text: string) {
    const prompt = text.trim();
    if (!prompt || busy || !selectedModel) return;

    setError(null);
    setDraft('');
    const userMsg: DisplayMessage = { id: nextId++, role: 'user', content: prompt };
    const assistantId = nextId++;
    setMessages((prev) => [
      ...prev,
      userMsg,
      { id: assistantId, role: 'assistant', content: '' },
    ]);
    setBusy(true);

    const history: ChatMessage[] = [...messages, userMsg].map(({ role, content }) => ({
      role,
      content,
    }));

    const controller = new AbortController();
    abortRef.current = controller;

    let assistantText = '';
    try {
      await streamChat(
        { model: selectedModel, messages: history, max_tokens: 512 },
        controller.signal,
        (delta) => {
          assistantText += delta;
          setMessages((prev) =>
            prev.map((m) =>
              m.id === assistantId ? { ...m, content: assistantText } : m,
            ),
          );
        },
        () => undefined,
      );
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (!controller.signal.aborted) {
        setError(message);
      }
      if (assistantText.length === 0) {
        setMessages((prev) => prev.filter((m) => m.id !== assistantId));
      }
    } finally {
      setMessages((prev) =>
        prev.map((m) =>
          m.id === assistantId && m.content === ''
            ? { ...m, content: assistantText || '_(empty response)_' }
            : m,
        ),
      );
      setBusy(false);
      abortRef.current = null;
    }
  }

  function stop() {
    abortRef.current?.abort();
  }

  return (
    <div className="glass-panel flex h-[calc(100vh-8.5rem)] flex-col rounded-2xl">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-white/8 px-5 py-3.5">
        <div className="flex items-center gap-2.5">
          <span className="flex h-8 w-8 items-center justify-center rounded-lg bg-gradient-to-br from-emerald-400/25 to-cyan-400/25 text-emerald-300">
            <Send className="h-4 w-4" />
          </span>
          <div>
            <h2 className="text-sm font-semibold text-slate-100">Cluster chat</h2>
            <p className="text-[11px] text-slate-500">
              streams through the pooled pipeline
            </p>
          </div>
        </div>
        <label className="flex items-center gap-2 text-xs text-slate-400">
          model
          <select
            value={selectedModel}
            onChange={(e) => setSelectedModel(e.target.value)}
            className="cursor-pointer rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 font-mono text-xs text-slate-200 outline-none transition-colors hover:border-emerald-400/40"
          >
            {models.length === 0 && <option value="">no models</option>}
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.id}
              </option>
            ))}
          </select>
        </label>
      </div>

      {error && (
        <div className="mx-5 mt-3 flex items-center gap-2 rounded-lg border border-red-400/30 bg-red-500/10 px-3 py-2 text-xs text-red-300">
          <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
          <span className="truncate">{error}</span>
        </div>
      )}

      <div className="flex-1 space-y-4 overflow-y-auto px-5 py-5">
        {messages.length === 0 && (
          <div className="flex h-full flex-col items-center justify-center text-center">
            <div className="mb-4 flex h-14 w-14 items-center justify-center rounded-2xl border border-emerald-400/30 bg-emerald-500/10">
              <Send className="h-6 w-6 text-emerald-300" />
            </div>
            <h3 className="text-base font-semibold text-slate-200">
              Start a conversation
            </h3>
            <p className="mt-1 max-w-sm text-sm text-slate-500">
              Responses stream token-by-token from the orchestrator’s OpenAI-compatible
              endpoint.
            </p>
            <div className="mt-5 flex flex-wrap justify-center gap-2">
              {SUGGESTIONS.map((s) => (
                <button
                  key={s}
                  onClick={() => void send(s)}
                  className="cursor-pointer rounded-full border border-white/10 bg-white/5 px-3.5 py-1.5 text-xs text-slate-300 transition-colors hover:border-emerald-400/40 hover:bg-emerald-500/10 hover:text-emerald-200"
                >
                  {s}
                </button>
              ))}
            </div>
          </div>
        )}

        <AnimatePresence initial={false}>
          {messages.map((m) => (
            <motion.div
              key={m.id}
              initial={{ opacity: 0, y: 12 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ type: 'spring', stiffness: 300, damping: 28 }}
              className={`flex ${m.role === 'user' ? 'justify-end' : 'justify-start'}`}
            >
              <div
                className={`max-w-[85%] rounded-2xl px-4 py-3 text-sm ${
                  m.role === 'user'
                    ? 'bg-emerald-500/15 border border-emerald-400/25 text-slate-100'
                    : 'bg-white/5 border border-white/8 text-slate-200'
                }`}
              >
                {m.role === 'assistant' ? (
                  <div className="markdown-body">
                    <ReactMarkdown>{m.content}</ReactMarkdown>
                    {busy && m === messages[messages.length - 1] && (
                      <span className="ml-0.5 inline-block h-4 w-2 translate-y-0.5 animate-blink-caret bg-emerald-400" />
                    )}
                  </div>
                ) : (
                  <div className="whitespace-pre-wrap">{m.content}</div>
                )}
              </div>
            </motion.div>
          ))}
        </AnimatePresence>
        <div ref={bottomRef} />
      </div>

      <div className="border-t border-white/8 px-5 py-4">
        <div className="flex items-end gap-2.5">
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                void send(draft);
              }
            }}
            rows={1}
            placeholder={
              apiOnline
                ? 'Message the cluster… (Enter to send)'
                : 'API offline — start the desktop app'
            }
            className="max-h-32 min-h-[44px] flex-1 resize-none rounded-xl border border-white/10 bg-white/5 px-4 py-3 text-sm text-slate-100 placeholder:text-slate-500 outline-none transition-colors focus:border-emerald-400/50"
          />
          {busy ? (
            <button
              onClick={stop}
              className="flex h-11 w-11 cursor-pointer items-center justify-center rounded-xl border border-red-400/40 bg-red-500/15 text-red-300 transition-colors hover:bg-red-500/25"
              aria-label="Stop generation"
            >
              <Square className="h-4 w-4" />
            </button>
          ) : (
            <button
              onClick={() => void send(draft)}
              disabled={!canSend}
              className="flex h-11 w-11 cursor-pointer items-center justify-center rounded-xl bg-gradient-to-br from-emerald-400 to-emerald-600 text-slate-950 shadow-[0_0_20px_rgba(16,185,129,0.35)] transition-all hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40 disabled:shadow-none"
              aria-label="Send message"
            >
              <Send className="h-4 w-4" />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
