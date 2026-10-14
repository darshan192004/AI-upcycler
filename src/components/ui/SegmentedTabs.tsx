import { motion, useReducedMotion } from 'motion/react';
import type { ReactNode } from 'react';

export interface TabItem<T extends string> {
  id: T;
  label: string;
  icon?: ReactNode;
}

interface SegmentedTabsProps<T extends string> {
  items: TabItem<T>[];
  active: T;
  onChange: (id: T) => void;
  layoutId: string;
  className?: string;
}

export function SegmentedTabs<T extends string>({
  items,
  active,
  onChange,
  layoutId,
  className = '',
}: SegmentedTabsProps<T>) {
  const reduceMotion = useReducedMotion();

  return (
    <div
      role="tablist"
      className={`flex items-center gap-1 rounded-xl border border-white/8 bg-white/4 p-1 ${className}`}
    >
      {items.map((item) => {
        const isActive = item.id === active;
        return (
          <button
            key={item.id}
            role="tab"
            aria-selected={isActive}
            onClick={() => onChange(item.id)}
            className={`relative flex cursor-pointer items-center gap-2 rounded-lg px-3.5 py-2 text-sm font-medium transition-colors duration-150 ${
              isActive ? 'text-slate-950' : 'text-slate-400 hover:text-slate-200'
            }`}
          >
            {isActive && (
              <motion.span
                layoutId={layoutId}
                className="absolute inset-0 rounded-lg bg-gradient-to-r from-emerald-400 to-emerald-500 shadow-[0_0_18px_rgba(16,185,129,0.35)]"
                transition={
                  reduceMotion
                    ? { duration: 0 }
                    : { type: 'spring', stiffness: 420, damping: 34 }
                }
              />
            )}
            <span className="relative z-10 flex items-center gap-2">
              {item.icon}
              {item.label}
            </span>
          </button>
        );
      })}
    </div>
  );
}
