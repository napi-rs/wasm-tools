import { test, expect } from '@playwright/test'

// Surface browser-side failures (worker load errors, wasm faults, 404s) in the
// Playwright output so a non-passing run is debuggable rather than opaque.
function wireLogs(page: import('@playwright/test').Page) {
  page.on('console', (m) => console.log(`[browser:${m.type()}] ${m.text()}`))
  page.on('pageerror', (e) => console.log(`[pageerror] ${e.message}`))
}

test('playground is cross-origin isolated and inspects a module into a graph', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')

  // SharedArrayBuffer / wasm threads require the COOP+COEP isolation the dev
  // middleware (and void.json in prod) stamp on /playground.
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  // The default example is the exported `add` module. Inspect compiles the WAT
  // (wabt) and parses it with wasm-tools in the worker, then renders the graph.
  await page.getByRole('button', { name: 'Inspect module' }).click()

  // The graph figure renders once the worker returns; assert on the type
  // signature, which appears in the graph but never in the example dropdown.
  await expect(page.getByRole('img', { name: /module graph/i })).toBeVisible({ timeout: 60_000 })
  await expect(page.getByText(/\(i32, i32\)\s*→\s*i32/).first()).toBeVisible({ timeout: 60_000 })
})

test('a module with a call renders the fn→fn "calls" edge', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  // The env.log sample: local `$run` calls the imported `$log`. buildGraph reads
  // `$run`'s body via instructions(), extracts the Call target, and (since the
  // imported function is itself a node in the functions collection) emits a
  // function→function "calls" edge that GraphView labels on its routed curve.
  await page.getByLabel('Example').selectOption({ label: 'imported env.log' })
  await page.getByRole('button', { name: 'Inspect module' }).click()

  await expect(page.getByRole('img', { name: /module graph/i })).toBeVisible({ timeout: 60_000 })
  await expect(page.locator('svg text', { hasText: /^calls$/ }).first()).toBeVisible({ timeout: 60_000 })
})

test('build mode composes add(a,b) from an IR tree and runs it', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  await page.getByRole('button', { name: 'build', exact: true }).click()
  await page.getByRole('button', { name: 'Build & run' }).click()

  // add(2,3) -> 5, and fn.instructions() reads the body back (round-trip proof).
  await expect(page.getByText(/=\s*5\b/).first()).toBeVisible({ timeout: 60_000 })
  await expect(page.getByText(/I32Add/).first()).toBeVisible({ timeout: 60_000 })
})
