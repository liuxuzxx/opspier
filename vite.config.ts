import { defineConfig } from 'vite';
export default defineConfig({
 server: { host: '127.0.0.1', port: 14962, strictPort: true },
 build: { rollupOptions: {
  onwarn(warning, warn) { if (warning.code === 'MODULE_LEVEL_DIRECTIVE' && warning.id?.includes('node_modules')) return; warn(warning); },
  output: { manualChunks: { terminal: ['@xterm/xterm','@xterm/addon-fit'], antd: ['antd'], react: ['react','react-dom'] } },
 }, chunkSizeWarningLimit: 1200 },
});
