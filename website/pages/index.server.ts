import { defineHandler, defineHead, type InferProps } from 'void'
import { highlight } from '../lib/highlight'
import { inspectSample, editSample, buildSample, storySample, builderSample } from './_data/samples'

// index.html must never be served from cache — keep it pure SSR (no prerender, plus
// revalidate:0 in void.json) so a deploy is live on the very next request. The shiki
// highlighter is a module-level singleton, so per-request rendering stays cheap.
export const prerender = false

export const loader = defineHandler(async () => ({
  inspectHtml: await highlight(inspectSample),
  editHtml: await highlight(editSample),
  buildHtml: await highlight(buildSample),
  storyHtml: await highlight(storySample),
  builderHtml: await highlight(builderSample),
}))

export type Props = InferProps<typeof loader>

const DESCRIPTION =
  'walrus bindings for JavaScript — read, edit and build WebAssembly modules through live handles, then emit real wasm.'

export const head = defineHead<Props>((c) => {
  // Derive the origin from the REQUEST rather than hardcoding it. The previous
  // constant pointed at https://wasm-tools.napi.rs, which does not resolve (NXDOMAIN),
  // so every unfurler that follows og:url — Facebook, LinkedIn, Slack — crawled a dead
  // host and fell back to a bare link. This page is `prerender = false`, so the handler
  // runs per request and the origin is always the host actually being served.
  const origin = new URL(c.req.url).origin
  return {
    title: 'See the shape of your wasm',
    link: [{ rel: 'canonical', href: `${origin}/` }],
    meta: [
      { name: 'description', content: DESCRIPTION },
      { property: 'og:type', content: 'website' },
      { property: 'og:site_name', content: '@napi-rs/wasm-tools' },
      { property: 'og:url', content: `${origin}/` },
      { property: 'og:title', content: '@napi-rs/wasm-tools' },
      { property: 'og:description', content: DESCRIPTION },
      // summary_large_image promises an image; without one X renders no card at all.
      { property: 'og:image', content: `${origin}/og.png` },
      { property: 'og:image:width', content: '1200' },
      { property: 'og:image:height', content: '630' },
      { property: 'og:image:alt', content: 'See the shape of your wasm — @napi-rs/wasm-tools' },
      { name: 'twitter:card', content: 'summary_large_image' },
      { name: 'twitter:image', content: `${origin}/og.png` },
    ],
  }
})
