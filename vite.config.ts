import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// https://vitejs.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: [
        "**/src-tauri/**",
        "**/.playwright-mcp/**",
        // Gradle output. Its native-library strip task holds locks on the .so
        // files while this watcher runs, which crashes vite with EBUSY
        // (observed on libconscrypt_jni.so). Nothing under here is an input to
        // the desktop app.
        "**/mobile-worker/android/**/build/**",
        "**/mobile-worker/android/.gradle/**",
      ],
    },
  },
}));
