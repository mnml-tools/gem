import { fileURLToPath, URL } from 'node:url';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
      // The kit's `tauri` entry imports two optional peers that only its updater uses; Gem has
      // no updater (plan §10.2), so they resolve to a stub (src/app/kitOptionalPlugins.ts).
      '@tauri-apps/plugin-process': fileURLToPath(
        new URL('./src/app/kitOptionalPlugins.ts', import.meta.url),
      ),
      '@tauri-apps/plugin-updater': fileURLToPath(
        new URL('./src/app/kitOptionalPlugins.ts', import.meta.url),
      ),
    },
  },
  // Tauri expects a fixed port and must see Rust errors in the terminal. Gem's own port (1426,
  // HMR 1427) lets it run next to Zap and Piw, which use 1420 (ADR 0001).
  clearScreen: false,
  server: {
    port: 1426,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1427 } : undefined,
    watch: { ignored: ['**/src-tauri/**', '**/dev/servers/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    // WebView2 (Chromium), WebKitGTK and WKWebView on macOS 12+ all support ES2022.
    target: ['es2022', 'chrome111', 'safari15'],
    minify: process.env.TAURI_ENV_DEBUG ? false : 'oxc',
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    chunkSizeWarningLimit: 800,
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.{ts,tsx}'],
    setupFiles: ['src/test/setup.ts'],
    restoreMocks: true,
    css: false,
    // Inline the kit so the optional-plugin aliases above also apply inside it.
    server: { deps: { inline: ['@mnml-tools/ui'] } },
  },
});
