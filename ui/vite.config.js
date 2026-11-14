import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// https://vite.dev/config/
// The Car Thing's on-device Chromium is an old embedded build (Chrome 69,
// confirmed live via CDP UA string) that doesn't support modern JS syntax
// (optional chaining, nullish coalescing, etc.) that esbuild/Vite emit
// untranspiled by default. Target it explicitly so the bundle actually runs
// in that browser instead of failing to parse as a module silently.
export default defineConfig({
  plugins: [react()],
  build: {
    target: 'chrome69',
  },
})
