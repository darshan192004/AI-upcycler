import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AnimatePresence, motion, useReducedMotion } from 'motion/react';
import ReactMarkdown from 'react-markdown';
import {
  AlertTriangle,
  CircleHelp,
  Cloud,
  Cpu,
  Send,
  Smartphone,
  Square,
} from 'lucide-react';
import {
  deleteSession,
  getMultimodalEnabled,
  getSettings,
  listSessionMessages,
  listSessions,
  renameSession,
  streamChat,
  uploadImage,
} from '../lib/api';
import type {
  AppSettings,
  ChatContent,
  ChatMessage,
  ChatSession,
  MediaRef,
  ProviderStatus,
} from '../lib/types';
import { useClusterStore } from '../store/cluster';
import { SettingsPanel } from './SettingsPanel';

type ProviderChoice = '' | 'local' | ProviderStatus['id'];
type SourceKind = 'local' | 'worker' | 'cloud' | 'unknown';

interface DisplayMessage extends ChatMessage {
  id: string;
}

const SUGGESTIONS = [
  'Explain how layer partitioning works',
  'What is AI Upcycler?',
  'Write a haiku about distributed computing',
];

/** The badge is derived from telemetry only; it never guesses a local state. */
function describeSource(
  source: string | null,
  providers: ProviderStatus[],
): { label: string; kind: SourceKind } {
  if (!source) return { label: 'Source unknown', kind: 'unknown' };
  if (source === 'local') return { label: 'Local engine', kind: 'local' };
  if (source.startsWith('adb:')) {
    return { label: `Worker ${source.slice(4)}`, kind: 'worker' };
  }
  const provider = providers.find((p) => p.id === source);
  return provider
    ? { label: `Cloud ${provider.name}`, kind: 'cloud' }
    : { label: 'Source unknown', kind: 'unknown' };
}

/** Older session rows hold plain text; image parts get a readable label. */
function contentToText(content: ChatContent): string {
  if (typeof content === 'string') return content;
  return content
    .map((part) => (part.type === 'text' ? part.text : `[image: ${part.file_name}]`))
    .join('\n');
}

export function ChatView() {
  const models = useClusterStore((s) => s.models);
  const providers = useClusterStore((s) => s.providers);
  const refreshModels = useClusterStore((s) => s.refreshModels);
  const refreshProviders = useClusterStore((s) => s.refreshProviders);
  const apiOnline = useClusterStore((s) => s.apiOnline);
  const source = useClusterStore((s) => s.latestMetrics?.source ?? null);

  const [sessionId, setSessionId] = useState<string>(() => crypto.randomUUID());
  const [selectedProvider, setSelectedProvider] = useState<ProviderChoice>('');
  const [sessions, setSessions] = useState<ChatSession[]>([]);
  const [selectedModel, setSelectedModel] = useState('');
  const [messages, setMessages] = useState<DisplayMessage[]>([]);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [loadingSession, setLoadingSession] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<MediaRef[]>([]);
  const [multimodalEnabled, setMultimodalEnabled] = useState(false);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [sessionTitleDraft, setSessionTitleDraft] = useState('');

  const localSeqRef = useRef(0);
  const sessionIdRef = useRef(sessionId);
  const sessionRequestRef = useRef(0);
  const firstTurnRef = useRef(false);
  const abortRef = useRef<AbortController | null>(null);
  const bottomRef = useRef<HTMLDivElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const reduceMotion = useReducedMotion();

  const loadSessions = useCallback(async () => {
    try {
      const list = await listSessions();
      setSessions(list.sessions);
    } catch {
      // Offline: keep the last list we saw rather than inventing sessions.
    }
  }, []);

  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  useEffect(() => {
    if (models.length === 0) void refreshModels();
  }, [models.length, refreshModels]);

  useEffect(() => {
    void refreshProviders();
    void loadSessions();
    void getSettings()
      .then(setSettings)
      .catch(() => {
        // Offline: generation falls back to the built-in 512-token limit.
      });
  }, [refreshProviders, loadSessions]);

  useEffect(() => {
    let live = true;
    void getMultimodalEnabled().then((enabled) => {
      if (live) setMultimodalEnabled(enabled);
    });
    return () => {
      live = false;
    };
  }, []);

  const modelOptions = useMemo(() => {
    const options: { id: string }[] = models.map((m) => ({ id: m.id }));
    const seen = new Set(options.map((o) => o.id));
    for (const provider of providers) {
      for (const model of provider.models) {
        const id = `${provider.id}:${model.id}`;
        if (seen.has(id)) continue;
        seen.add(id);
        options.push({ id });
      }
    }
    return options;
  }, [models, providers]);

  const sessionOptions = useMemo(() => {
    const byId = new Map(sessions.map((s) => [s.session_id, s]));
    if (!byId.has(sessionId)) {
      byId.set(sessionId, {
        session_id: sessionId,
        title: 'New session',
        model_id: selectedModel,
        created_at: '',
      });
    }
    return [...byId.values()];
  }, [sessions, sessionId, selectedModel]);

  const sourceInfo = useMemo(
    () => describeSource(source, providers),
    [source, providers],
  );

  /** Rename/delete act on the selected row, never on the unsaved new session. */
  const savedSession = sessions.find((s) => s.session_id === sessionId) ?? null;

  useEffect(() => {
    if (modelOptions.length === 0) {
      if (selectedModel !== '') setSelectedModel('');
      return;
    }
    if (!modelOptions.some((option) => option.id === selectedModel)) {
      setSelectedModel(modelOptions[0].id);
    }
  }, [modelOptions, selectedModel]);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: reduceMotion ? 'auto' : 'smooth' });
  }, [messages, reduceMotion]);

  const canSend = useMemo(
    () =>
      (draft.trim().length > 0 || attachments.length > 0) &&
      !busy &&
      !loadingSession &&
      selectedModel.length > 0,
    [draft, attachments.length, busy, loadingSession, selectedModel],
  );

  function nextLocalId(): string {
    localSeqRef.current += 1;
    return `local-${localSeqRef.current}`;
  }

  function chooseProvider(value: ProviderChoice) {
    setSelectedProvider(value);
    if (value === '' || value === 'local') {
      if (models[0]) setSelectedModel(models[0].id);
      return;
    }
    const virtual = modelOptions.find((option) => option.id.startsWith(`${value}:`));
    if (virtual) setSelectedModel(virtual.id);
  }

  async function selectSession(id: string) {
    const requestId = ++sessionRequestRef.current;
    setSessionId(id);
    sessionIdRef.current = id;
    setError(null);
    setRenaming(false);
    firstTurnRef.current = false;
    abortRef.current?.abort();
    abortRef.current = null;
    setBusy(false);
    setMessages([]);
    setLoadingSession(Boolean(id));
    if (!id) {
      setLoadingSession(false);
      return;
    }
    try {
      const thread = await listSessionMessages(id);
      if (requestId !== sessionRequestRef.current) return;
      setMessages(
        thread.messages.map((m) => ({
          id: `session-${m.message_id}`,
          role: m.role,
          content: m.content,
        })),
      );
    } catch (err) {
      if (requestId !== sessionRequestRef.current) return;
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      setMessages([]);
    } finally {
      if (requestId === sessionRequestRef.current) setLoadingSession(false);
    }
  }

  function startNewSession() {
    sessionRequestRef.current += 1;
    const nextId = crypto.randomUUID();
    sessionIdRef.current = nextId;
    setSessionId(nextId);
    setError(null);
    setRenaming(false);
    setSessionTitleDraft('');
    abortRef.current?.abort();
    abortRef.current = null;
    setBusy(false);
    setLoadingSession(false);
    setMessages([]);
    firstTurnRef.current = false;
  }

  async function saveSessionTitle() {
    const title = sessionTitleDraft.trim();
    if (!title || !savedSession) {
      setRenaming(false);
      return;
    }
    try {
      setError(null);
      const updated = await renameSession(sessionId, title);
      setSessions((prev) =>
        prev.map((s) => (s.session_id === updated.session_id ? updated : s)),
      );
      setRenaming(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function removeSession() {
    const removedId = sessionId;
    try {
      setError(null);
      await deleteSession(removedId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return;
    }
    setSessions((prev) => prev.filter((s) => s.session_id !== removedId));
    if (sessionIdRef.current !== removedId) return;
    sessionRequestRef.current += 1;
    abortRef.current?.abort();
    abortRef.current = null;
    const nextId = crypto.randomUUID();
    sessionIdRef.current = nextId;
    setSessionId(nextId);
    setMessages([]);
    setRenaming(false);
    setSessionTitleDraft('');
    setLoadingSession(false);
    firstTurnRef.current = false;
  }

  async function attachFile(file: File | undefined) {
    if (!file) return;
    setAttachmentError(null);
    try {
      const ref = await uploadImage(file);
      setAttachments((xs) => [...xs, ref]);
    } catch {
      setAttachmentError('Invalid image attachment');
    }
  }

  async function send(text: string) {
    const prompt = text.trim();
    if (
      (!prompt && attachments.length === 0) ||
      busy ||
      loadingSession ||
      !selectedModel
    ) {
      return;
    }

    // Zero attachments keeps the historical string payload; otherwise the
    // message becomes a part array matching the host's MessageContent union.
    const outgoing: ChatContent =
      attachments.length === 0
        ? prompt
        : [
            { type: 'text', text: prompt },
            ...attachments.map((a) => ({ type: 'image' as const, ...a })),
          ];

    setError(null);
    setDraft('');
    setAttachments([]);
    setAttachmentError(null);
    const userMsg: DisplayMessage = { id: nextLocalId(), role: 'user', content: outgoing };
    const assistantId = nextLocalId();
    setMessages((prev) => [
      ...prev,
      userMsg,
      { id: assistantId, role: 'assistant', content: '' },
    ]);
    setBusy(true);

    const userWire: ChatMessage = { role: 'user', content: outgoing };
    const history: ChatMessage[] = [
      ...messages.map(({ role, content }) => ({ role, content })),
      userWire,
    ];
    const request = {
      model: selectedModel,
      messages: history,
      max_tokens: settings?.max_tokens ?? 512,
      session_id: sessionId,
      ...(selectedProvider !== '' ? { provider: selectedProvider } : {}),
    };

    const controller = new AbortController();
    abortRef.current = controller;

    let assistantText = '';
    try {
      await streamChat(
        request,
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
      if (!controller.signal.aborted && !firstTurnRef.current) {
        firstTurnRef.current = true;
        await loadSessions();
      }
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
      if (abortRef.current === controller) {
        setBusy(false);
        abortRef.current = null;
      }
    }
  }

  function stop() {
    abortRef.current?.abort();
  }

  const SourceIcon =
    sourceInfo.kind === 'local'
      ? Cpu
      : sourceInfo.kind === 'worker'
        ? Smartphone
        : sourceInfo.kind === 'cloud'
          ? Cloud
          : CircleHelp;

  return (
    <div className="glass-panel flex h-[calc(100vh-8.5rem)] flex-col rounded-2xl">
      <div className="flex flex-wrap items-end justify-between gap-3 border-b border-white/8 px-5 py-3.5">
        <div className="flex items-center gap-2.5">
          <span className="flex h-8 w-8 items-center justify-center rounded-lg bg-gradient-to-br from-emerald-400/25 to-cyan-400/25 text-emerald-300">
            <Send className="h-4 w-4" aria-hidden="true" />
          </span>
          <div>
            <h2 className="text-sm font-semibold text-slate-100">Cluster chat</h2>
            <p className="text-[11px] text-slate-500">streams through the pooled pipeline</p>
          </div>
        </div>

        <div className="chat-controls">
          <label className="chat-field">
            <span>Provider</span>
            <select
              aria-label="Provider"
              className="chat-select"
              value={selectedProvider}
              onChange={(e) => chooseProvider(e.target.value as ProviderChoice)}
            >
              <option value="">Auto</option>
              <option value="local">Local</option>
              {providers.map((provider) => (
                <option key={provider.id} value={provider.id}>
                  {provider.name}
                </option>
              ))}
            </select>
          </label>

          <label className="chat-field">
            <span>Model</span>
            <select
              aria-label="Model"
              className="chat-select"
              value={selectedModel}
              onChange={(e) => setSelectedModel(e.target.value)}
            >
              {modelOptions.length === 0 && <option value="">no models</option>}
              {modelOptions.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.id}
                </option>
              ))}
            </select>
          </label>

          <label className="chat-field">
            <span>Session</span>
            <select
              aria-label="Chat session"
              className="chat-select"
              value={sessionId}
              onChange={(e) => void selectSession(e.target.value)}
            >
              {sessionOptions.map((session) => (
                <option key={session.session_id} value={session.session_id}>
                  {session.title}
                </option>
              ))}
            </select>
          </label>

          <span
            className={`chat-source-badge chat-source-${sourceInfo.kind}`}
            role="status"
            aria-live="polite"
          >
            <SourceIcon className="h-3.5 w-3.5" aria-hidden="true" />
            <span>{sourceInfo.label}</span>
          </span>

          <div className="chat-field">
            <span>Session actions</span>
            <div className="flex items-center gap-1.5">
              <button
                type="button"
                aria-label="New session"
                onClick={startNewSession}
                className="min-h-11 rounded-lg border border-[#475569] bg-[#1B2336] px-2 text-xs text-slate-200 transition-colors hover:border-emerald-400/50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E]"
              >
                New
              </button>
              <button
                type="button"
                aria-label="Rename session"
                disabled={!savedSession || renaming}
                onClick={() => {
                  setSessionTitleDraft(savedSession?.title ?? '');
                  setRenaming(true);
                }}
                className="min-h-11 rounded-lg border border-[#475569] bg-[#1B2336] px-2 text-xs text-slate-200 transition-colors hover:border-emerald-400/50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E] disabled:cursor-not-allowed disabled:opacity-40"
              >
                Rename
              </button>
              <button
                type="button"
                aria-label="Delete session"
                disabled={!savedSession}
                onClick={() => void removeSession()}
                className="min-h-11 rounded-lg border border-[#475569] bg-[#1B2336] px-2 text-xs text-slate-200 transition-colors hover:border-red-400/50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E] disabled:cursor-not-allowed disabled:opacity-40"
              >
                Delete
              </button>
              <button
                type="button"
                aria-label="Open generation settings"
                aria-expanded={settingsOpen}
                onClick={() => setSettingsOpen((open) => !open)}
                className="min-h-11 rounded-lg border border-[#475569] bg-[#1B2336] px-2 text-xs text-slate-200 transition-colors hover:border-emerald-400/50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E]"
              >
                Settings
              </button>
            </div>
          </div>
        </div>
      </div>

      {renaming && (
        <div className="flex flex-wrap items-end gap-2 border-b border-white/8 px-5 py-3">
          <label className="chat-field min-w-0 flex-1">
            <span>Session title</span>
            <input
              aria-label="Session title"
              type="text"
              value={sessionTitleDraft}
              onChange={(e) => setSessionTitleDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  void saveSessionTitle();
                }
              }}
              className="mt-1 min-h-11 w-full rounded-lg border border-[#475569] bg-[#0F172A] px-2 py-1 text-sm text-slate-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E]"
            />
          </label>
          <button
            type="button"
            aria-label="Save session title"
            onClick={() => void saveSessionTitle()}
            className="min-h-11 rounded-lg bg-[#22C55E] px-3 text-xs font-semibold text-[#0F172A] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E]"
          >
            Save title
          </button>
          <button
            type="button"
            aria-label="Cancel rename"
            onClick={() => setRenaming(false)}
            className="min-h-11 rounded-lg border border-[#475569] px-3 text-xs text-slate-200 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E]"
          >
            Cancel
          </button>
        </div>
      )}

      {settingsOpen && (
        <div className="border-b border-white/8 px-5 py-3">
          <SettingsPanel settings={settings} onSettingsChange={setSettings} />
        </div>
      )}

      {error && (
        <div
          id="chat-error"
          role="alert"
          className="mx-5 mt-3 flex items-center gap-2 rounded-lg border border-red-400/30 bg-red-500/10 px-3 py-2 text-xs text-red-300"
        >
          <AlertTriangle className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          <span className="truncate">{error}</span>
        </div>
      )}

      <div
        className="flex-1 space-y-4 overflow-y-auto px-5 py-5"
        role="log"
        aria-label="Chat messages"
        aria-busy={busy || loadingSession}
      >
        {messages.length === 0 && (
          <div className="flex h-full flex-col items-center justify-center text-center">
            <div className="mb-4 flex h-14 w-14 items-center justify-center rounded-2xl border border-emerald-400/30 bg-emerald-500/10">
              <Send className="h-6 w-6 text-emerald-300" aria-hidden="true" />
            </div>
            <h3 className="text-base font-semibold text-slate-200">Start a conversation</h3>
            <p className="mt-1 max-w-sm text-sm text-slate-500">
              Responses stream token-by-token from the orchestrator’s OpenAI-compatible endpoint.
            </p>
            <div className="mt-5 flex flex-wrap justify-center gap-2">
              {SUGGESTIONS.map((s) => (
                <button
                  key={s}
                  type="button"
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
              initial={reduceMotion ? false : { opacity: 0, y: 12 }}
              animate={{ opacity: 1, y: 0 }}
              transition={
                reduceMotion ? { duration: 0 } : { type: 'spring', stiffness: 300, damping: 28 }
              }
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
                    <ReactMarkdown>{contentToText(m.content)}</ReactMarkdown>
                    {busy && m === messages[messages.length - 1] && (
                      <span
                        className="ml-0.5 inline-block h-4 w-2 translate-y-0.5 animate-blink-caret bg-emerald-400"
                        aria-hidden="true"
                      />
                    )}
                  </div>
                ) : (
                  <div className="whitespace-pre-wrap">{contentToText(m.content)}</div>
                )}
              </div>
            </motion.div>
          ))}
        </AnimatePresence>
        <div ref={bottomRef} />
      </div>

      <div className="border-t border-white/8 px-5 py-4">
        <style>{`
          .attachment-chip-list {
            display: flex;
            flex-wrap: wrap;
            gap: 0.5rem;
            list-style: none;
            margin: 0 0 0.75rem;
            padding: 0;
          }
          .attachment-chip {
            display: inline-flex;
            align-items: center;
            gap: 0.5rem;
            border-radius: 0.5rem;
            padding: 0.35rem 0.6rem;
            font-size: 0.75rem;
            transition: background 200ms ease;
          }
          .attachment-chip button {
            border: 1px solid #475569;
            border-radius: 0.375rem;
            background: transparent;
            color: #94A3B8;
            cursor: pointer;
            line-height: 1;
            padding: 0 0.35rem;
            transition: background 200ms ease, color 200ms ease;
          }
          .attachment-chip button:hover {
            background: #EF4444;
            color: #F8FAFC;
          }
          @media (prefers-reduced-motion: reduce) {
            .attachment-chip,
            .attachment-chip button { transition: none; }
          }
        `}</style>
        {multimodalEnabled && (
          <label className="chat-field mb-3">
            <span>Attach image</span>
            <input
              ref={fileInputRef}
              type="file"
              accept="image/png,image/jpeg,image/webp,image/gif"
              aria-label="Attach image"
              onChange={(e) => {
                void attachFile(e.target.files?.[0]);
                e.target.value = '';
              }}
            />
          </label>
        )}
        {attachmentError && (
          <p role="alert" className="mb-2 text-xs text-red-300">
            {attachmentError}
          </p>
        )}
        {attachments.length > 0 && (
          <ul aria-label="Image attachments" className="attachment-chip-list">
            {attachments.map((a) => (
              <li
                key={a.media_hash}
                className="attachment-chip"
                style={{
                  background: '#1B2336',
                  border: '1px solid #475569',
                  color: '#94A3B8',
                }}
              >
                <span>{a.file_name} · {a.mime_type} · {a.byte_count} B</span>
                <button
                  type="button"
                  aria-label={`Remove ${a.file_name}`}
                  onClick={() =>
                    setAttachments((xs) => xs.filter((x) => x.media_hash !== a.media_hash))
                  }
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        )}
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
            aria-label="Message"
            aria-describedby={error ? 'chat-error' : undefined}
            placeholder={
              apiOnline
                ? 'Message the cluster… (Enter to send)'
                : 'API offline — start the desktop app'
            }
            className="max-h-32 min-h-[44px] flex-1 resize-none rounded-xl border border-white/10 bg-white/5 px-4 py-3 text-sm text-slate-100 placeholder:text-slate-500 outline-none transition-colors focus:border-emerald-400/50"
          />
          {busy ? (
            <button
              type="button"
              onClick={stop}
              className="chat-send flex h-11 w-11 cursor-pointer items-center justify-center rounded-xl border border-red-400/40 bg-red-500/15 text-red-300 transition-colors hover:bg-red-500/25 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E]"
              aria-label="Stop generation"
            >
              <Square className="h-4 w-4" aria-hidden="true" />
            </button>
          ) : (
            <button
              type="button"
              onClick={() => void send(draft)}
              disabled={!canSend}
              className="chat-send flex h-11 w-11 cursor-pointer items-center justify-center rounded-xl bg-gradient-to-br from-emerald-400 to-emerald-600 text-slate-950 shadow-[0_0_20px_rgba(16,185,129,0.35)] transition-all hover:brightness-110 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E] disabled:cursor-not-allowed disabled:opacity-40 disabled:shadow-none"
              aria-label="Send message"
            >
              <Send className="h-4 w-4" aria-hidden="true" />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
