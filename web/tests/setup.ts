import '@testing-library/jest-dom/vitest'

// The version string is injected by Vite's `define` at build time. In tests we
// provide it as a global so `src/lib/version.ts` resolves it at runtime and
// tests can control it via vi.stubGlobal.
;(globalThis as Record<string, unknown>).__APP_VERSION__ = '0.0.0'
