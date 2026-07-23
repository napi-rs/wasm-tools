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

test('an extended-const global inspects (WAT features mirror the walrus parser)', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  // `(global i32 (i32.add …))` only parses/validates under extended_const. walrus accepts
  // the emitted binary, so the WAT path must enable the same flag; without it, wabt would
  // reject this and the inspect would error instead of rendering the global node.
  await page.getByLabel('Example').selectOption({ label: 'extended-const global' })
  await page.getByRole('button', { name: 'Inspect module' }).click()

  await expect(page.getByRole('img', { name: /module graph/i })).toBeVisible({ timeout: 60_000 })
  await expect(page.getByText(/answer/).first()).toBeVisible({ timeout: 60_000 })
})

test('clearing an export name is transmitted, not silently dropped', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  await page.getByRole('button', { name: 'edit', exact: true }).click()
  await page.getByLabel('Example').selectOption({ label: 'memory + mutable global + exports' })
  await page.getByRole('button', { name: 'Inspect to edit' }).click()

  // Edit form is ready once the memory (number) input renders.
  await expect(page.locator('input[type="number"]').first()).toBeVisible({ timeout: 60_000 })

  // The first export-name field (text input #1; #0 is the module name). Clearing it to ''
  // is a valid, binding-accepted rename — diffEdits must register it as a pending edit
  // rather than dropping the empty string, and Apply must produce downloadable bytes.
  const firstExport = page.locator('input[type="text"]').nth(1)
  await firstExport.fill('')
  await expect(page.getByText(/1 pending/)).toBeVisible()

  await page.getByRole('button', { name: /Apply edits/ }).click()
  await expect(page.getByRole('button', { name: 'Download .wasm' })).toBeVisible({ timeout: 60_000 })
})

test('an invalid memory edit blocks Apply even alongside a valid edit', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  await page.getByRole('button', { name: 'edit', exact: true }).click()
  await page.getByLabel('Example').selectOption({ label: 'memory + mutable global + exports' })
  await page.getByRole('button', { name: 'Inspect to edit' }).click()

  const memInput = page.locator('input[type="number"]').first()
  await expect(memInput).toBeVisible({ timeout: 60_000 })

  // A valid export rename alone would enable Apply…
  await page.locator('input[type="text"]').nth(1).fill('renamed')
  await expect(page.getByRole('button', { name: /Apply edits/ })).toBeEnabled()

  // …but a changed-yet-invalid memory value must block Apply, so no download can be
  // produced that contradicts the shown form. The field error surfaces the reason.
  await memInput.fill('1.5')
  await expect(page.getByText(/Memory pages must be a whole number/i)).toBeVisible()
  await expect(page.getByRole('button', { name: /Apply edits/ })).toBeDisabled()

  // Correcting the memory value (within the sample's max of 2) re-enables Apply.
  await memInput.fill('2')
  await expect(page.getByRole('button', { name: /Apply edits/ })).toBeEnabled()
})

test('a failed re-inspection clears the stale edit session and download', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  await page.getByRole('button', { name: 'edit', exact: true }).click()
  await page.getByLabel('Example').selectOption({ label: 'memory + mutable global + exports' })
  await page.getByRole('button', { name: 'Inspect to edit' }).click()

  const memInput = page.locator('input[type="number"]').first()
  await expect(memInput).toBeVisible({ timeout: 60_000 })
  // Apply a valid edit → a downloadable binary for module A appears.
  await memInput.fill('2')
  await page.getByRole('button', { name: /Apply edits/ }).click()
  await expect(page.getByRole('button', { name: 'Download .wasm' })).toBeVisible({ timeout: 60_000 })

  // Replace the source with malformed WAT and re-inspect: the parse fails.
  await page.getByLabel('WAT source').fill('(module (this is not valid wat')
  await page.getByRole('button', { name: 'Inspect to edit' }).click()

  // The stale A session must be gone: no Download button, and the edit form is cleared
  // back to the "inspect first" prompt (so A can't be applied/downloaded under source B).
  await expect(page.getByText(/Inspect a module first/i)).toBeVisible({ timeout: 60_000 })
  await expect(page.getByRole('button', { name: 'Download .wasm' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: /Apply edits/ })).toHaveCount(0)
})

test('changing the source invalidates the applied edit session (no stale Apply/Download)', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  await page.getByRole('button', { name: 'edit', exact: true }).click()
  await page.getByLabel('Example').selectOption({ label: 'memory + mutable global + exports' })
  await page.getByRole('button', { name: 'Inspect to edit' }).click()

  const memInput = page.locator('input[type="number"]').first()
  await expect(memInput).toBeVisible({ timeout: 60_000 })
  // Apply a valid edit → module A becomes downloadable.
  await memInput.fill('2')
  await page.getByRole('button', { name: /Apply edits/ }).click()
  await expect(page.getByRole('button', { name: 'Download .wasm' })).toBeVisible({ timeout: 60_000 })

  // Editing the displayed source (to a different but VALID module) must invalidate the
  // whole A session immediately — with NO re-inspect — so A's edit form can't be applied
  // and A's emitted bytes can't be downloaded behind an editor that now shows B.
  await page.getByLabel('WAT source').fill('(module)')
  await expect(page.getByText(/Inspect a module first/i)).toBeVisible()
  await expect(page.getByRole('button', { name: 'Download .wasm' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: /Apply edits/ })).toHaveCount(0)
})

test('uploading a binary supersedes the WAT session and deactivates the editor', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  await page.getByRole('button', { name: 'edit', exact: true }).click()
  await page.getByLabel('Example').selectOption({ label: 'memory + mutable global + exports' })
  await page.getByRole('button', { name: 'Inspect to edit' }).click()

  const memInput = page.locator('input[type="number"]').first()
  await expect(memInput).toBeVisible({ timeout: 60_000 })
  // Apply a valid edit → the WAT module (A) becomes downloadable.
  await memInput.fill('2')
  await page.getByRole('button', { name: /Apply edits/ }).click()
  await expect(page.getByRole('button', { name: 'Download .wasm' })).toBeVisible({ timeout: 60_000 })

  // Upload a different module as a binary (the minimal 8-byte empty module). Selecting it
  // must supersede A's session: A's Download is gone, and the WAT editor is deactivated
  // (it still holds A's text, which cannot represent this binary).
  await page.locator('input[type="file"]').setInputFiles({
    name: 'empty.wasm',
    mimeType: 'application/wasm',
    buffer: Buffer.from([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]),
  })
  await expect(page.getByRole('button', { name: 'Download .wasm' })).toHaveCount(0)
  await expect(page.getByLabel('WAT source')).toBeDisabled()
  // The binary is the active source now; its filename identifies it.
  await expect(page.getByText('empty.wasm').first()).toBeVisible({ timeout: 60_000 })

  // Switching back re-activates the WAT editor with its preserved source.
  await page.getByRole('button', { name: /Switch to WAT editor/i }).click()
  await expect(page.getByLabel('WAT source')).toBeEnabled()
})

test('a failed binary read returns to the WAT editor (no stuck deactivated editor)', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  // Upload bytes that are not a valid module (bad magic). The read succeeds but the parse
  // fails: the session must NOT stay in the deactivated 'binary loaded' state — it reverts
  // to the WAT editor so the source controls work again and no false banner lingers.
  await page.locator('input[type="file"]').setInputFiles({
    name: 'bad.wasm',
    mimeType: 'application/wasm',
    buffer: Buffer.from([0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
  })
  await expect(page.getByLabel('WAT source')).toBeEnabled({ timeout: 60_000 })
  await expect(page.getByRole('button', { name: /Switch to WAT editor/i })).toHaveCount(0)
})

test('a funcref table renders a table node in the graph', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  // The table sample compiles + parses through the worker; buildGraph's table block must
  // emit a table node without erroring. Its sub label is the element type, which valTypeLabel
  // renders as "(ref null func)" — asserted on the SVG node text (not the dropdown option).
  await page.getByLabel('Example').selectOption({ label: 'funcref table + call_indirect' })
  await page.getByRole('button', { name: 'Inspect module' }).click()

  await expect(page.getByRole('img', { name: /module graph/i })).toBeVisible({ timeout: 60_000 })
  await expect(page.locator('svg text', { hasText: /ref null func/ }).first()).toBeVisible({ timeout: 60_000 })
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

test('build mode validates numeric args (no silent coercion)', async ({ page }) => {
  wireLogs(page)
  await page.goto('/playground')
  await expect.poll(() => page.evaluate(() => self.crossOriginIsolated), { timeout: 30_000 }).toBe(true)

  await page.getByRole('button', { name: 'build', exact: true }).click()
  const firstArg = page.locator('input[type="number"]').first()

  // Empty, non-integer, and rounding-prone exponent forms are all rejected (Build
  // disabled), never silently coerced to a different value that runs a different call.
  await firstArg.fill('')
  await expect(page.getByText(/whole i32 number/i)).toBeVisible()
  await expect(page.getByRole('button', { name: /Build & run/ })).toBeDisabled()

  await firstArg.fill('1.5')
  await expect(page.getByRole('button', { name: /Build & run/ })).toBeDisabled()

  // "1e-999" would round to 0 under Number(); the exact-integer parser rejects it.
  await firstArg.fill('1e-999')
  await expect(page.getByRole('button', { name: /Build & run/ })).toBeDisabled()

  // A plain integer re-enables Build.
  await firstArg.fill('1000')
  await expect(page.getByRole('button', { name: /Build & run/ })).toBeEnabled()
})
