import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { fileURLToPath, URL } from 'node:url'

const host = process.env.TAURI_DEV_HOST

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],

  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },

  // Tauri expects a fixed port and fails if it is not available. 1420 is
  // Tauri's default and collides with other Tauri projects, so claim our own.
  clearScreen: false,
  server: {
    port: 1428,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1429 } : undefined,
    watch: {
      // src-tauri is watched by the Rust side; watching it here causes double reloads.
      ignored: ['**/src-tauri/**'],
    },
  },

  build: {
    // Tauri ships a modern webview on every platform we target.
    target: 'esnext',
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    // Vite 8 minifies with oxc; the esbuild path is deprecated and needs a
    // separate esbuild install.
    minify: !process.env.TAURI_ENV_DEBUG,
    // Deliberately no `manualChunks` for Monaco. A manual chunk splits the
    // *file* but not the import graph, so Monaco stayed a static import of the
    // entry — preloaded and executed before React mounted, on every launch.
    // Worse, naming the chunk pinned it into the entry's graph even after the
    // import became dynamic. Letting rolldown split on the dynamic import in
    // `components/lazy-editor.tsx` keeps ~3.9MB off the startup path entirely.
  },
})
