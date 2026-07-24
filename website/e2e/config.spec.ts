import { test, expect } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

// Config-drift guard. Cross-origin isolation for /playground is served two
// different ways: the dev/preview server replays headers via the vite.config
// middleware (this is what the e2e suite actually exercises), while PRODUCTION
// serves them per-route from void.json. The e2e run never hits the production
// path, so without this spec the two could silently diverge — a deploy would
// ship a /playground that can't enable SharedArrayBuffer even though every
// browser test was green. Parse both sources and assert they agree.

const read = (rel: string) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8')

// void.json: routing.headers[route] is an array of "Name: value" strings.
function voidHeaders(route: string): Record<string, string> {
  const cfg = JSON.parse(read('../void.json')) as {
    routing: { headers: Record<string, string[]> }
  }
  const lines = cfg.routing.headers[route]
  expect(lines, `void.json is missing headers for "${route}"`).toBeTruthy()
  const out: Record<string, string> = {}
  for (const line of lines) {
    const i = line.indexOf(':')
    out[line.slice(0, i).trim()] = line.slice(i + 1).trim()
  }
  return out
}

// vite.config.ts: the dev/preview middleware sets headers via res.setHeader('Name', 'value').
function middlewareHeaders(): Record<string, string> {
  const src = read('../vite.config.ts')
  const out: Record<string, string> = {}
  const re = /res\.setHeader\(\s*'([^']+)'\s*,\s*'([^']+)'\s*\)/g
  let m: RegExpExecArray | null
  while ((m = re.exec(src))) out[m[1]] = m[2]
  return out
}

const COOP = 'Cross-Origin-Opener-Policy'
const COEP = 'Cross-Origin-Embedder-Policy'
const CORP = 'Cross-Origin-Resource-Policy'

test('dev middleware and prod void.json agree on /playground isolation headers', () => {
  const dev = middlewareHeaders()
  const prod = voidHeaders('/playground')

  // The two headers cross-origin isolation actually requires on the document.
  expect(dev[COOP]).toBe('same-origin')
  expect(dev[COEP]).toBe('require-corp')
  expect(prod[COOP]).toBe('same-origin')
  expect(prod[COEP]).toBe('require-corp')

  // Prod must not drift from what the e2e-tested dev server provides.
  expect(prod[COOP]).toBe(dev[COOP])
  expect(prod[COEP]).toBe(dev[COEP])
})

test('/playground/* subroute headers match the /playground document headers', () => {
  expect(voidHeaders('/playground/*')).toEqual(voidHeaders('/playground'))
})

test('/assets/* carries the COEP+CORP that lets the hashed worker asset load under require-corp', () => {
  // A dedicated worker spawned by a require-corp document is itself blocked unless
  // its own response carries COEP: require-corp (see worker.ts header note); the
  // dev middleware stamps CORP: same-origin on every response, so prod assets must too.
  const dev = middlewareHeaders()
  const assets = voidHeaders('/assets/*')
  expect(assets[COEP]).toBe('require-corp')
  expect(assets[CORP]).toBe('same-origin')
  expect(assets[CORP]).toBe(dev[CORP])
})
