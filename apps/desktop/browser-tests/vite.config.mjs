import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
const path = (value) => fileURLToPath(new URL(value, import.meta.url));
export default defineConfig({
  root: path('./'),
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@devo/ui': path('../packages/ui/src'), '@': path('../src/renderer') } },
  server: { host: '127.0.0.1', port: 1421, strictPort: true, fs: { allow: [path('../')] } },
  build: { outDir: 'dist' },
});
