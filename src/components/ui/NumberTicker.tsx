import { animate, useReducedMotion } from 'motion/react';
import { useEffect, useRef, useState } from 'react';

interface NumberTickerProps {
  value: number;
  decimals?: number;
  duration?: number;
  className?: string;
}

export function NumberTicker({
  value,
  decimals = 1,
  duration = 0.9,
  className,
}: NumberTickerProps) {
  const reduceMotion = useReducedMotion();
  const [display, setDisplay] = useState(value);
  const previous = useRef(value);

  useEffect(() => {
    const from = previous.current;
    previous.current = value;
    if (reduceMotion || from === value) {
      setDisplay(value);
      return;
    }
    const controls = animate(from, value, {
      duration,
      ease: [0.16, 1, 0.3, 1],
      onUpdate: (latest) => setDisplay(latest),
    });
    return () => controls.stop();
  }, [value, reduceMotion, duration]);

  return <span className={className}>{display.toFixed(decimals)}</span>;
}
