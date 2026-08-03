// The version string is injected at build time by Vite's `define`
// (see vite.config.ts), so no package metadata reaches the client bundle.
declare const __APP_VERSION__: string

export const APP_VERSION = `v${__APP_VERSION__ ?? '0.0.0'}`
