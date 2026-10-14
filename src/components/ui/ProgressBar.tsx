interface ProgressBarProps {
  value: number;
  className?: string;
  barClassName?: string;
  shimmer?: boolean;
  ariaLabel?: string;
}

export function ProgressBar({
  value,
  className = '',
  barClassName = 'bg-gradient-to-r from-emerald-500 to-cyan-400',
  shimmer = false,
  ariaLabel,
}: ProgressBarProps) {
  const pct = Math.max(0, Math.min(1, value)) * 100;
  return (
    <div
      role="progressbar"
      aria-valuenow={Math.round(pct)}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-label={ariaLabel}
      className={`h-2 w-full overflow-hidden rounded-full bg-white/8 ${className}`}
    >
      <div
        className={`h-full rounded-full transition-[width] duration-700 ease-out ${barClassName} ${
          shimmer ? 'shimmer-sweep' : ''
        }`}
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}
