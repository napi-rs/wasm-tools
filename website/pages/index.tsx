import type { Props } from './index.server'
import Hero from './_components/Hero'
import Verbs from './_components/Verbs'
import LiveHandleStory from './_components/LiveHandleStory'
import BuilderHighlight from './_components/BuilderHighlight'
import ApiMap from './_components/ApiMap'
import CtaBand from './_components/CtaBand'

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
