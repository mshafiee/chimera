/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        background: '#1A1A1A',
        surface: '#242424',
        'surface-light': '#2E2E2E',
        border: '#3A3A3A',
        shield: {
          DEFAULT: '#00D4FF',
          dark: '#0099CC',
        },
        spear: {
          DEFAULT: '#FF8800',
          dark: '#FF6600',
        },
        profit: '#00FF88',
        loss: '#FF4444',
        text: {
          DEFAULT: '#E0E0E0',
          muted: '#888888',
        },
      },
      fontFamily: {
        mono: ['JetBrains Mono', 'monospace'],
        sans: ['Inter', 'system-ui', 'sans-serif'],
      },
    },
  },
  plugins: [],
}
