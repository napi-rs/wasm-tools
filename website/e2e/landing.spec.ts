import { test, expect } from '@playwright/test'

test('landing renders the hero, CTA, and install command', async ({ page }) => {
  await page.goto('/')

  await expect(page.getByRole('heading', { name: /see the shape of your wasm/i })).toBeVisible()
  await expect(page.getByRole('link', { name: /open playground/i }).first()).toBeVisible()
  // the install switcher shows the package install command
  await expect(page.getByText(/add @napi-rs\/wasm-tools/).first()).toBeVisible()
})

// Everything below the hero is wrapped in <Reveal>, which starts at opacity 0 (via
// `.js .reveal`) and only becomes visible when its IntersectionObserver effect runs.
// That effect runs ONLY if the landing page hydrates. Playwright counts an opacity-0
// element as "visible", so the assertions above would still pass on a page that never
// hydrated and was blank below the fold — these two specs are what actually prove it.
test('sections below the hero reveal on scroll (landing page hydrates)', async ({ page }) => {
  await page.goto('/')
  await expect(page.locator('.reveal').first()).toBeAttached()

  for (let i = 0; i < 12; i++) {
    await page.mouse.wheel(0, 800)
    await page.waitForTimeout(150)
  }

  await expect
    .poll(() => page.evaluate(() => document.querySelectorAll('.reveal.is-visible').length), {
      timeout: 15_000,
    })
    .toBeGreaterThan(5)
  // and they are actually painted, not just class-tagged
  const opacity = await page.evaluate(
    () => getComputedStyle(document.querySelector('.reveal.is-visible')!).opacity,
  )
  expect(opacity).toBe('1')
})

test('the install switcher swaps the package-manager command', async ({ page }) => {
  await page.goto('/')
  await expect(page.getByText(/pnpm add @napi-rs\/wasm-tools/).first()).toBeVisible()

  await page.getByRole('button', { name: 'yarn', exact: true }).first().click()
  await expect(page.getByText(/yarn add @napi-rs\/wasm-tools/).first()).toBeVisible()

  await page.getByRole('button', { name: 'npm', exact: true }).first().click()
  await expect(page.getByText(/npm i -D @napi-rs\/wasm-tools/).first()).toBeVisible()
})
