import { type ReactNode } from 'react'

// Explicit, ordered docs navigation. Kept as a hand-maintained list (rather than
// auto-discovered from @void/md/pages) so the order and labels are deterministic —
// docs read top-to-bottom, and that order is editorial, not alphabetical.
//
// No active-link highlight: these are fully server-rendered markdown pages that
// don't hydrate, and a nested layout's router only exposes its mounted base
// ('/docs'), not the leaf route — so the current page can't be resolved here
// without client JS the page never ships. The sidebar is a plain nav list.
// The root pages/layout.tsx already supplies the header + footer; this nested
// layout only owns the docs shell (sidebar + prose column).
const NAV = [
  { href: '/docs', label: 'Getting Started' },
  { href: '/docs/module-graph', label: 'The module graph' },
  { href: '/docs/value-types', label: 'Value types & constants' },
  { href: '/docs/building-functions', label: 'Building functions' },
  { href: '/docs/api-reference', label: 'API Reference' },
]

export default function DocsLayout({ children }: { children: ReactNode }) {
  return (
    <div className="container-page py-12 md:py-16">
      {/* Mobile doc nav. Native <details> — no JS, so it works on these
          non-hydrating markdown pages. Replaced by the sidebar at md+. */}
      <details className="mb-8 rounded-xl border border-(--color-border) bg-(--color-surface-1) md:hidden">
        <summary className="flex cursor-pointer list-none items-center justify-between px-4 py-3 font-mono text-xs uppercase tracking-wider text-(--color-muted)">
          Documentation
          <span className="nav-caret text-(--color-faint)" aria-hidden="true">
            ▾
          </span>
        </summary>
        <nav className="flex flex-col gap-1 px-2 pb-2 text-sm">
          {NAV.map((item) => (
            <a
              key={item.href}
              href={item.href}
              className="flex min-h-11 items-center rounded-md px-2 text-(--color-muted) transition-colors hover:bg-(--color-surface-2) hover:text-(--color-fg)"
            >
              {item.label}
            </a>
          ))}
        </nav>
      </details>
      <div className="flex gap-10 lg:gap-16">
        <aside className="hidden w-56 shrink-0 md:block">
          <nav className="sticky top-24 flex flex-col gap-1 text-sm">
            <p className="mb-3 font-mono text-xs uppercase tracking-wider text-(--color-muted) opacity-60">
              Documentation
            </p>
            {NAV.map((item) => (
              <a
                key={item.href}
                href={item.href}
                className="border-l-2 border-(--color-border) py-1 pl-3 text-(--color-muted) transition-colors hover:border-(--color-accent) hover:text-(--color-fg)"
              >
                {item.label}
              </a>
            ))}
          </nav>
        </aside>
        <article className="void-md min-w-0 flex-1">{children}</article>
      </div>
    </div>
  )
}
