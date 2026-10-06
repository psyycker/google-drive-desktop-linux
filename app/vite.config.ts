import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// The Tauri shell embeds `dist/` (see src-tauri/tauri.conf.json); `cargo tauri dev`
// loads this dev server instead.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    target: 'es2022',
  },
});
