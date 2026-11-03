import { createReadStream, existsSync, statSync } from 'node:fs'
import { extname, join, normalize } from 'node:path'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig, type Plugin } from 'vite'

// Serves dev-mock/ at /mock/ under `npm run dev` only, so sample recordings
// used for browser-only UI work never end up in the app bundle.
function devMock(): Plugin {
  const root = join(__dirname, 'dev-mock')
  const types: Record<string, string> = { '.json': 'application/json', '.png': 'image/png', '.mp4': 'video/mp4' }
  return {
    name: 'openscrib-dev-mock',
    apply: 'serve',
    configureServer(server) {
      server.middlewares.use('/mock', (req, res, next) => {
        const file = normalize(join(root, decodeURIComponent((req.url ?? '/').split('?')[0])))
        if (!file.startsWith(root) || !existsSync(file) || statSync(file).isDirectory()) return next()
        res.setHeader('content-type', types[extname(file)] ?? 'application/octet-stream')
        createReadStream(file).pipe(res)
      })
    },
  }
}

// Tauri expects a fixed port and doesn't want Vite to clear its output.
export default defineConfig({
  plugins: [svelte(), devMock()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
})
