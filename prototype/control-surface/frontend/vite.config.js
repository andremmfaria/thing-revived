import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// https://vite.dev/config/
// The Car Thing's on-device Chromium is an old embedded build (Chrome 69,
// confirmed live via CDP UA string) that doesn't support modern JS syntax
// (optional chaining, nullish coalescing, etc.) that esbuild/Vite emit
// untranspiled by default. Target it explicitly so the bundle actually runs
// in that browser instead of failing to parse as a module silently.
// Vite's HTML plugin always emits <script type="module" crossorigin ...>
// for the entry point regardless of the Rollup output format, so even an
// IIFE bundle still gets tagged as a module script in index.html. Strip
// those attributes post-build so the tag is a plain <script src="...">.
function stripModuleScriptTag() {
  return {
    name: 'strip-module-script-tag',
    transformIndexHtml: {
      order: 'post',
      handler(html) {
        // type="module" scripts are deferred by spec; a plain classic
        // <script> in <head> runs immediately, before <div id="root"> in
        // <body> exists, so getElementById('root') would be null. Add
        // defer explicitly to preserve the original execution timing.
        return html
          .replace(/<script type="module" crossorigin /g, '<script defer ')
          .replace(/<link rel="stylesheet" crossorigin /g, '<link rel="stylesheet" ')
      },
    },
  }
}

export default defineConfig({
  plugins: [react(), stripModuleScriptTag()],
  // Served on-device as a file:// URL (Chromium kiosk pointed directly at
  // index.html on local disk, not via an http server), so asset paths must
  // be relative to index.html's own directory, not root-relative ("/assets/..."
  // resolves against the filesystem root under file://, not the webapp dir).
  base: './',
  build: {
    target: 'chrome69',
    // file:// URLs serve no Content-Type header, so Chromium's strict MIME
    // checking for type="module" scripts fails ("non-JavaScript MIME type
    // of \"\""), leaving the page blank. Force a plain IIFE bundle (single
    // entry, no code-splitting) so index.html gets a bare <script src="...">
    // with no type="module"/crossorigin, sidestepping the MIME requirement.
    rollupOptions: {
      output: {
        format: 'iife',
        entryFileNames: 'assets/[name]-[hash].js',
      },
    },
  },
})
