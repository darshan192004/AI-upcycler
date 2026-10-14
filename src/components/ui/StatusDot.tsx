const TONE: Record<string, string> = {
  ok: 'bg-emerald-400 shadow-[0_0_10px_rgba(52,211,153,0.8)]',
  warn: 'bg-amber-400 shadow-[0_0_10px_rgba(251,191,36,0.8)]',
  error: 'bg-red-400 shadow-[0_0_10px_rgba(248,113,113,0.8)]',
  idle: 'bg-slate-500',
};

interface StatusDotProps {
  tone?: keyof typeof TONE;
  pulse?: boolean;
  className?: string;
}

export function StatusDot({ tone = 'ok', pulse = true, className = '' }: StatusDotProps) {
  return (
    <span
      aria-hidden="true"
      className={`inline-block h-2 w-2 rounded-full ${TONE[tone]} ${
        pulse ? 'animate-pulse' : ''
      } ${className}`}
    />
  );
}
