import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
const path = (value) => fileURLToPath(new URL(value, import.meta.url));
export default defineConfig({
  root: path('./'),
  plugins: [
    {
      name: 'skill-fixture-data-boundary',
      enforce: 'pre',
      resolveId(source, importer) {
        if (source === '../../services/connection-manager' &&
            importer?.replaceAll('\\', '/').endsWith('/components/chat/skill-picker-dialog.tsx')) {
          return '\0skill-fixture-connection';
        }
      },
      load(id) {
        if (id === '\0skill-fixture-connection') {
          // The data boundary only handles the error fixture directory; other calls throw.
          return `export { getProjectClient } from ${JSON.stringify(path("./skill-error-data.ts"))};`;
        }
      },
    },
    react(), tailwindcss(),
  ],
  resolve: { alias: { '@devo/ui': path('../packages/ui/src'), '@': path('../src/renderer') } },
  server: { host: '127.0.0.1', port: 1421, strictPort: true, fs: { allow: [path('../')] } },
  build: { outDir: 'dist' },
});
