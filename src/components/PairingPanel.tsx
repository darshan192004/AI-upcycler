import { useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { toDataURL } from 'qrcode';
import { Check, Copy, QrCode } from 'lucide-react';
import { GlowCard } from './ui/GlowCard';

/** What `prepare_pairing` hands back (spec 2026-10-20, D-B8). */
type PairingSession = {
  pin: string;
  qr_payload: string;
  expires_in_secs: number;
  fingerprint: string;
};

const inputClass =
  'rounded-xl border border-white/10 bg-white/5 px-4 py-2.5 font-mono text-xs text-slate-100 placeholder:text-slate-500 outline-none transition-colors focus:border-cyan-400/50';

export function PairingPanel() {
  const [serial, setSerial] = useState('');
  const [session, setSession] = useState<PairingSession | null>(null);
  const [qrDataUrl, setQrDataUrl] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const start = async () => {
    const device = serial.trim();
    if (!device) return;
    setBusy(true);
    setError(null);
    try {
      if (!isTauri()) {
        throw new Error(
          'Pairing runs inside the desktop app — open AI Upcycler from its Tauri shell, not the browser.',
        );
      }
      const next = await invoke<PairingSession>('prepare_pairing', { serial: device });
      const qr = await toDataURL(next.qr_payload, { margin: 1, width: 220 });
      setSession(next);
      setQrDataUrl(qr);
      setCopied(false);
    } catch (e) {
      setSession(null);
      setQrDataUrl(null);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const copyUri = async () => {
    if (!session) return;
    try {
      await navigator.clipboard.writeText(session.qr_payload);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setError('Clipboard unavailable — select the pairing URI and copy it manually.');
    }
  };

  const expiresIn = session
    ? `single-use · expires in ${Math.max(1, Math.round(session.expires_in_secs / 60))} min`
    : null;

  return (
    <GlowCard className="p-5" hover={false}>
      <div className="mb-3 flex items-center gap-2">
        <h2 className="text-sm font-semibold uppercase tracking-wider text-slate-300">
          Pair a phone worker
        </h2>
        <QrCode className="h-4 w-4 text-cyan-400" />
      </div>

      <p className="mb-4 text-xs text-slate-400">
        Bridge the pairing listener to a USB-attached Android device, then paste the
        pairing URI into the phone app (or type the PIN and the first 8 fingerprint
        characters). The session is single-use.
      </p>

      <div className="flex flex-col gap-3 md:flex-row">
        <label htmlFor="pairing-serial" className="sr-only">
          ADB device serial
        </label>
        <input
          id="pairing-serial"
          value={serial}
          onChange={(e) => setSerial(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void start();
          }}
          placeholder="adb serial (e.g. R3CN30ABCDE)"
          className={`${inputClass} flex-1`}
        />
        <button
          onClick={() => void start()}
          disabled={!serial.trim() || busy}
          className="flex cursor-pointer items-center justify-center gap-2 rounded-xl border border-emerald-400/30 bg-emerald-500/10 px-5 py-2.5 text-xs font-semibold text-emerald-300 transition-colors hover:border-emerald-400/60 hover:bg-emerald-500/20 disabled:cursor-not-allowed disabled:border-white/8 disabled:bg-white/5 disabled:text-slate-500"
        >
          {busy ? 'Bridging device…' : session ? 'New session' : 'Create session'}
        </button>
      </div>

      {error && (
        <p role="alert" className="mt-3 text-xs text-red-300">
          {error}
        </p>
      )}

      {session && qrDataUrl && (
        <div className="mt-5 grid grid-cols-1 gap-5 border-t border-white/10 pt-5 md:grid-cols-[auto_1fr]">
          <div className="mx-auto rounded-xl bg-white p-3 md:mx-0">
            <img
              src={qrDataUrl}
              alt="Pairing QR code — or copy the pairing URI below"
              className="h-44 w-44"
            />
          </div>

          <div className="flex min-w-0 flex-col gap-3">
            <div>
              <span className="text-[11px] font-semibold uppercase tracking-wider text-slate-400">
                Pairing PIN
              </span>
              <p className="font-mono text-4xl font-semibold tracking-[0.3em] text-emerald-300">
                {session.pin}
              </p>
              <p className="mt-1 text-[11px] text-slate-500">{expiresIn}</p>
            </div>

            <div>
              <span className="text-[11px] font-semibold uppercase tracking-wider text-slate-400">
                Master CA fingerprint
              </span>
              <p className="break-all font-mono text-[11px] text-slate-300">
                {session.fingerprint}
              </p>
            </div>

            <div>
              <label
                htmlFor="pairing-uri"
                className="text-[11px] font-semibold uppercase tracking-wider text-slate-400"
              >
                Pairing URI (paste into the phone app)
              </label>
              <div className="mt-1 flex gap-2">
                <input
                  id="pairing-uri"
                  readOnly
                  value={session.qr_payload}
                  onFocus={(e) => e.target.select()}
                  className={`${inputClass} min-w-0 flex-1 text-[11px] text-slate-300`}
                />
                <button
                  onClick={() => void copyUri()}
                  className="flex shrink-0 cursor-pointer items-center gap-1.5 rounded-xl border border-emerald-400/30 bg-emerald-500/10 px-3 text-xs font-semibold text-emerald-300 transition-colors hover:border-emerald-400/60 hover:bg-emerald-500/20"
                  aria-label="Copy pairing URI"
                >
                  {copied ? (
                    <>
                      <Check className="h-3.5 w-3.5" /> Copied
                    </>
                  ) : (
                    <>
                      <Copy className="h-3.5 w-3.5" /> Copy
                    </>
                  )}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </GlowCard>
  );
}
