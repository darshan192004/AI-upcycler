import { motion } from 'motion/react';
import type { ReactNode } from 'react';

interface GlowCardProps {
  children: ReactNode;
  glow?: boolean;
  hover?: boolean;
  className?: string;
}

export function GlowCard({
  children,
  glow = true,
  hover = true,
  className = '',
}: GlowCardProps) {
  return (
    <motion.div
      whileHover={hover ? { y: -3 } : undefined}
      transition={{ type: 'spring', stiffness: 380, damping: 28 }}
      className={`glass-panel relative rounded-2xl ${glow ? 'glow-border' : ''} ${className}`}
    >
      {children}
    </motion.div>
  );
}
