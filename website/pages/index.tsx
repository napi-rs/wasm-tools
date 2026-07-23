import type { Props } from './index.server'
import Hero from './_components/hero'
import Verbs from './_components/verbs'
import LiveHandleStory from './_components/live-handle-story'
import BuilderHighlight from './_components/builder-highlight'
import ApiMap from './_components/api-map'
import CtaBand from './_components/cta-band'

export default function Home({
  inspectHtml,
  editHtml,
  buildHtml,
  storyHtml,
  builderHtml,
}: Props) {
  return (
    <>
      <Hero />
      <Verbs inspectHtml={inspectHtml} editHtml={editHtml} buildHtml={buildHtml} />
      <LiveHandleStory html={storyHtml} />
      <BuilderHighlight html={builderHtml} />
      <ApiMap />
      <CtaBand />
    </>
  )
}
