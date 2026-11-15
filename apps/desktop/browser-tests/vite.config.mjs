import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
const path = (value) => fileURLToPath(new URL(value, import.meta.url));
export default defineConfig({
  root: path('./'),
  plugins: [{
    name: 'automation-test-host-boundaries', enforce: 'pre',
    resolveId(source, importer) {
      if (!importer?.includes('/src/renderer/components/automations/')) return;
      if (['@tanstack/react-router', '../../hooks/use-devo-data', '../../services/backend', '../chat/prompt-toolbar'].includes(source))
        return path('./automation-boundaries.tsx');
    },
  }, react(), tailwindcss()],
  resolve: { alias: { '@devo/ui': path('../packages/ui/src'), '@': path('../src/renderer') } },
  server: { host: '127.0.0.1', port: 1421, strictPort: true, fs: { allow: [path('../')] } },
  build: { outDir: 'dist' },
});
