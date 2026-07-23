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

test('a failed re-apply removes the stale download (edit-mode integrity)', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  await page.getByRole('button', { name: 'edit', exact: true }).click()
  // This sample has `(memory $mem 1 2)` — initial 1, max 2.
  await page.getByLabel('Example').selectOption({ label: 'memory + mutable global + exports' })
  await page.getByRole('button', { name: 'Inspect to edit' }).click()

  const initial = page.locator('input[type="number"]').first()
  await expect(initial).toBeVisible({ timeout: 60_000 })

  // Valid edit (1 → 2, within max 2): applies and produces a downloadable binary.
  await initial.fill('2')
  await page.getByRole('button', { name: /Apply edits/ }).click()
  await expect(page.getByRole('button', { name: 'Download .wasm' })).toBeVisible({ timeout: 60_000 })

  // Invalid re-apply (2 → 3 exceeds max 2): the emit fails on re-parse, and the
  // previous binary must NOT remain downloadable (it wouldn't match the form).
  await initial.fill('3')
  await page.getByRole('button', { name: /Apply edits/ }).click()
  await expect(page.getByText(/minimum|maximum|error/i).first()).toBeVisible({ timeout: 60_000 })
  await expect(page.getByRole('button', { name: 'Download .wasm' })).toHaveCount(0)
})

test('re-inspecting resets the edit form (no stale pending edits carried over)', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  await page.getByRole('button', { name: 'edit', exact: true }).click()
  await page.getByLabel('Example').selectOption({ label: 'memory + mutable global + exports' })
  await page.getByRole('button', { name: 'Inspect to edit' }).click()

  const initial = page.locator('input[type="number"]').first()
  await expect(initial).toBeVisible({ timeout: 60_000 })
  await initial.fill('2') // dirty the form
  await expect(page.getByText(/1 pending/)).toBeVisible()

  // A fresh inspect must commit the new module and its form together — leaving the
  // previous form paired with the new result would carry stale pending edits over.
  await page.getByRole('button', { name: 'Inspect to edit' }).click()
  await expect(page.getByText(/0 pending/)).toBeVisible({ timeout: 60_000 })
})

test('a shared-memory module inspects (wabt validate gets the threads feature)', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  // `(memory 1 1 shared)` only validates under the threads feature. If the worker's
  // validate() ran with wabt's baseline features (the bug), this inspect would error
  // instead of rendering a graph. The memory node's `shared: true` prop is the proof.
  await page.getByLabel('Example').selectOption({ label: 'shared memory (threads)' })
  await page.getByRole('button', { name: 'Inspect module' }).click()

  await expect(page.getByRole('img', { name: /module graph/i })).toBeVisible({ timeout: 60_000 })
  await expect(page.getByText(/1\.\.1 pages/).first()).toBeVisible({ timeout: 60_000 })
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
