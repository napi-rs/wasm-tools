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

const SITE_URL = 'https://wasm-tools.napi.rs'
const DESCRIPTION =
  'walrus bindings for JavaScript — read, edit and build WebAssembly modules through live handles, then emit real wasm.'

export const head = defineHead<Props>(() => ({
  title: 'See the shape of your wasm',
  meta: [
    { name: 'description', content: DESCRIPTION },
    { property: 'og:type', content: 'website' },
    { property: 'og:site_name', content: '@napi-rs/wasm-tools' },
    { property: 'og:url', content: `${SITE_URL}/` },
    { property: 'og:title', content: '@napi-rs/wasm-tools' },
    { property: 'og:description', content: DESCRIPTION },
    { name: 'twitter:card', content: 'summary_large_image' },
  ],
}))
