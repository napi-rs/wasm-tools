import { test, expect } from '@playwright/test'

test('landing renders the hero, CTA, and install command', async ({ page }) => {
  await page.goto('/')

  await expect(page.getByRole('heading', { name: /see the shape of your wasm/i })).toBeVisible()
  await expect(page.getByRole('link', { name: /open playground/i }).first()).toBeVisible()
  // the install switcher shows the package install command
  await expect(page.getByText(/add @napi-rs\/wasm-tools/).first()).toBeVisible()
})
