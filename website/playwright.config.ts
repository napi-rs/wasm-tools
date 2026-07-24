import { defineConfig } from '@playwright/test'

const BASE_URL = process.env.PLAYWRIGHT_BASE_URL ?? 'http://localhost:5173'

export default defineConfig({
  testDir: './e2e',
  timeout: 120_000,
  // Serial. The playground spec cold-compiles the ~2.3 MB wasm-tools wasm build
  // (256 MB shared memory + wabt init), which saturates every core; running the
  // specs in parallel starves each other's hydration/worker budgets. Serial is
  // reliably green and still fast.
  workers: 1,
  use: { baseURL: BASE_URL },
  // Self-contained: boot the Void dev server for the run. Locally we reuse an
  // already-running dev server for fast iteration; in CI we always boot a fresh
  // one so the gate can't pass against a stale server missing the vite.config
  // cross-origin-isolation middleware the playground depends on.
  webServer: {
    command: 'npm run dev',
    url: `${BASE_URL}/`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
})
