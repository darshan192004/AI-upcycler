import { useReducedMotion } from 'motion/react';

export function AuroraBackground() {
  const reduceMotion = useReducedMotion();

  return (
    <div className="pointer-events-none fixed inset-0 -z-10 overflow-hidden grain" aria-hidden="true">
      <div
        className={`absolute -top-40 -left-32 h-[520px] w-[520px] rounded-full opacity-40 blur-[110px] ${
          reduceMotion ? '' : 'animate-drift'
        }`}
        style={{
          background:
            'radial-gradient(circle at 30% 30%, rgba(16,185,129,0.55), transparent 65%)',
        }}
      />
      <div
        className={`absolute top-1/3 -right-40 h-[560px] w-[560px] rounded-full opacity-30 blur-[120px] ${
          reduceMotion ? '' : 'animate-drift'
        }`}
        style={{
          background:
            'radial-gradient(circle at 60% 40%, rgba(6,182,212,0.5), transparent 65%)',
          animationDelay: '-6s',
        }}
      />
      <div
        className={`absolute -bottom-48 left-1/3 h-[460px] w-[460px] rounded-full opacity-25 blur-[120px] ${
          reduceMotion ? '' : 'animate-drift'
        }`}
        style={{
          background:
            'radial-gradient(circle at 50% 50%, rgba(245,158,11,0.35), transparent 65%)',
          animationDelay: '-12s',
        }}
      />
      <svg
        className="absolute inset-0 h-full w-full opacity-[0.05]"
        viewBox="0 0 1440 900"
        preserveAspectRatio="xMidYMid slice"
      >
        <defs>
          <linearGradient id="blob" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0%" stopColor="#10B981" />
            <stop offset="100%" stopColor="#06B6D4" />
          </linearGradient>
        </defs>
        <path
          fill="url(#blob)"
          d="M420,140 C560,90 760,110 880,210 C1010,320 1050,500 960,620 C870,740 680,780 520,720 C360,660 270,520 290,380 C310,250 320,175 420,140 Z"
        />
        <path
          fill="url(#blob)"
          opacity="0.6"
          d="M980,480 C1080,430 1220,470 1280,570 C1340,670 1300,800 1200,850 C1100,900 960,870 900,780 C840,690 880,530 980,480 Z"
        />
      </svg>
      <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_center,transparent_0%,rgba(11,15,25,0.55)_75%,rgba(11,15,25,0.9)_100%)]" />
    </div>
  );
}
