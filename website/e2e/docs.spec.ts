import { test, expect } from '@playwright/test'

test('docs render with the sidebar and highlighted code', async ({ page }) => {
  await page.goto('/docs')

  await expect(page.getByRole('heading', { name: /getting started/i }).first()).toBeVisible()
  // sidebar lists the authored pages
  await expect(page.getByRole('link', { name: 'Building functions' }).first()).toBeVisible()
  await expect(page.getByRole('link', { name: 'API Reference' }).first()).toBeVisible()
  // an install command from a highlighted code block
  await expect(page.getByText('pnpm add @napi-rs/wasm-tools').first()).toBeVisible()
})
