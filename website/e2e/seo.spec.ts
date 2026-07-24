import { test, expect } from '@playwright/test'

// Regression guards for the SEO / LLM-agent / a11y defects found by auditing the
// deployed site. Each spec pins the specific thing that was broken, so the fix cannot
// silently regress. Measured numbers in the comments are the pre-fix values.

const DOCS = [
  '/docs',
  '/docs/api-reference',
  '/docs/building-functions',
  '/docs/module-graph',
  '/docs/value-types',
]

test('the landing head points at the host actually serving the page', async ({ page }) => {
  // og:url was hardcoded to https://wasm-tools.napi.rs, which does not resolve
  // (NXDOMAIN) — every unfurler that follows it crawled a dead host.
  await page.goto('/')
  const origin = new URL(page.url()).origin

  await expect(page.locator('meta[property="og:url"]')).toHaveAttribute('content', `${origin}/`)
  await expect(page.locator('link[rel=canonical]')).toHaveAttribute('href', `${origin}/`)
})

test('summary_large_image is backed by an image that resolves', async ({ page }) => {
  // The card type promised an image while zero og:image/twitter:image existed on any
  // route, so X rendered no card at all.
  await page.goto('/')
  await expect(page.locator('meta[name="twitter:card"]')).toHaveAttribute(
    'content',
    'summary_large_image',
  )

  const src = await page.locator('meta[property="og:image"]').getAttribute('content')
  expect(src).toBeTruthy()
  await expect(page.locator('meta[name="twitter:image"]')).toHaveAttribute('content', src!)

  const res = await page.request.get(src!)
  expect(res.status()).toBe(200)
  expect(res.headers()['content-type']).toContain('image/')
})

test('/playground describes itself without JavaScript', async ({ browser }) => {
  // The island is `load`-hydrated, so a crawler used to receive "Playground / Loading…"
  // — 20 characters, zero headings — and the hydrated page then reused the LANDING
  // page's h1. The heading + intro are now static markup in index.island.tsx.
  const ctx = await browser.newContext({ javaScriptEnabled: false })
  const page = await ctx.newPage()
  await page.goto('/playground')

  await expect(page.locator('h1')).toHaveCount(1)
  await expect(page.locator('h1')).toHaveText('WebAssembly playground')
  const text = await page.locator('main').innerText()
  expect(text.length).toBeGreaterThan(300)
  expect(text).toContain('Web Worker')

  await ctx.close()
})

test('the playground reserves its hydrated height (no layout shift)', async ({ page }) => {
  // Hydration used to expand the island ~484px a second after paint: CLS 0.35 desktop
  // / 0.47 mobile against 0 on every other route. Assert the pre-mount placeholder is
  // the same height as the mounted tool, which is what keeps CLS at 0.
  await page.setViewportSize({ width: 1280, height: 900 })
  const island = page.locator('main section > div').last()

  await page.route('**/assets/*_playground*', (r) => r.abort())
  await page.goto('/playground')
  await expect(page.getByText('Loading…')).toBeVisible()
  const reserved = (await island.boundingBox())!.height
  await page.unroute('**/assets/*_playground*')

  await page.goto('/playground')
  await page.getByRole('button', { name: 'Inspect module' }).waitFor({ timeout: 60_000 })
  const hydrated = (await island.boundingBox())!.height

  expect(Math.abs(hydrated - reserved)).toBeLessThan(24)
})

test('heading permalinks are not keyboard-focusable', async ({ page }) => {
  // @void/md hides them with opacity:0, which leaves them in the tab order: 26
  // invisible focus stops across the docs where the ring landed on blank space and
  // AT announced nothing (axe: aria-hidden-focus, serious). visibility:hidden is not
  // focusable. Hover must still reveal them.
  for (const route of DOCS) {
    await page.goto(route)
    const anchors = page.locator('a.header-anchor')
    expect(await anchors.count()).toBeGreaterThan(0)

    const focusable = await page.evaluate(
      () =>
        [...document.querySelectorAll('a.header-anchor')].filter(
          (a) => getComputedStyle(a).visibility !== 'hidden',
        ).length,
    )
    expect(focusable, `${route} has focusable aria-hidden anchors`).toBe(0)
  }

  await page.goto('/docs')
  await page.locator('h2').first().hover()
  await expect(page.locator('h2 a.header-anchor').first()).toBeVisible()
})

test('the install command is never clipped on a phone', async ({ page }) => {
  // `truncate` cut it to "$ pnpm add @napi-rs/..." at every common phone width — the
  // package name and the -D flag were unreadable, and zoom does not reflow a truncation.
  for (const width of [320, 360, 390]) {
    await page.setViewportSize({ width, height: 844 })
    await page.goto('/')

    const clipped = await page.evaluate(() => {
      const span = [...document.querySelectorAll('span')].find((s) =>
        /^pnpm add @napi-rs\/wasm-tools -D$/.test(s.textContent?.trim() ?? ''),
      )
      if (!span) return null
      return span.scrollWidth > span.clientWidth + 1
    })
    expect(clipped, `install command clipped at ${width}px`).toBe(false)
  }
})

test('a deep-linked heading clears the sticky header', async ({ page }) => {
  // Without scroll-padding-top every #anchor landed under the h-14 sticky header.
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/docs#install')
  await page.waitForTimeout(600)

  const { top, headerBottom } = await page.evaluate(() => ({
    top: document.getElementById('install')!.getBoundingClientRect().top,
    headerBottom: document.querySelector('header')!.getBoundingClientRect().bottom,
  }))
  expect(top).toBeGreaterThanOrEqual(headerBottom)
})

test('every nav landmark has an accessible name', async ({ page }) => {
  for (const route of ['/', '/docs', '/playground']) {
    await page.goto(route)
    const unnamed = await page.evaluate(
      () => [...document.querySelectorAll('nav')].filter((n) => !n.getAttribute('aria-label')).length,
    )
    expect(unnamed, `${route} has unnamed <nav> landmarks`).toBe(0)
  }
})
