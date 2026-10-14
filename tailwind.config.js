/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      colors: {
        surface: {
          DEFAULT: '#0F172A',
          raised: '#1B2336',
          muted: '#272F42',
        },
        accent: {
          DEFAULT: '#22C55E',
          emerald: '#10B981',
          cyan: '#06B6D4',
          amber: '#F59E0B',
        },
      },
      fontFamily: {
        sans: ['"Fira Sans"', 'Inter', 'system-ui', 'sans-serif'],
        mono: ['"Fira Code"', 'ui-monospace', 'Consolas', 'monospace'],
      },
      keyframes: {
        shimmer: {
          '0%': { transform: 'translateX(-100%)' },
          '100%': { transform: 'translateX(100%)' },
        },
        'blink-caret': {
          '0%, 100%': { opacity: '1' },
          '50%': { opacity: '0' },
        },
        drift: {
          '0%, 100%': { transform: 'translate3d(0,0,0) scale(1)' },
          '33%': { transform: 'translate3d(30px,-24px,0) scale(1.06)' },
          '66%': { transform: 'translate3d(-24px,18px,0) scale(0.95)' },
        },
        'sweep': {
          '0%': { backgroundPosition: '200% 0' },
          '100%': { backgroundPosition: '-200% 0' },
        },
      },
      animation: {
        shimmer: 'shimmer 1.8s ease-in-out infinite',
        'blink-caret': 'blink-caret 1s step-end infinite',
        drift: 'drift 18s ease-in-out infinite',
        sweep: 'sweep 2.4s linear infinite',
      },
    },
  },
  plugins: [],
};
