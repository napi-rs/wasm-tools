import { useEffect, useState } from 'react'
import CopyButton from './_CopyButton'

const cx = (...c: (string | false | undefined)[]) => c.filter(Boolean).join(' ')

const PMS = [
  { id: 'pnpm', cmd: 'pnpm add @napi-rs/wasm-tools -D' },
  { id: 'yarn', cmd: 'yarn add @napi-rs/wasm-tools -D' },
  { id: 'bun', cmd: 'bun add @napi-rs/wasm-tools -d' },
  { id: 'npm', cmd: 'npm i -D @napi-rs/wasm-tools' },
]

const STORAGE_KEY = 'napi-wasm-tools:pm'

export default function InstallSwitcher() {
  const [active, setActive] = useState('pnpm')

  useEffect(() => {
    if (typeof window === 'undefined') return
    try {
      const saved = window.localStorage.getItem(STORAGE_KEY)
      if (saved && PMS.some((p) => p.id === saved)) setActive(saved)
    } catch {}
  }, [])

  const onSelect = (id: string) => {
    setActive(id)
    try {
      window.localStorage.setItem(STORAGE_KEY, id)
    } catch {}
  }

  const activeCmd = (PMS.find((p) => p.id === active) ?? PMS[0]).cmd

  return (
    <div className="max-w-md">
      <div className="flex items-center gap-4 font-mono text-sm">
        {PMS.map((pm) => {
          const isActive = pm.id === active
          return (
            <button
              key={pm.id}
              type="button"
              onClick={() => onSelect(pm.id)}
              className={cx(
                'relative inline-flex min-h-10 items-center pb-2 transition-colors',
                isActive ? 'text-(--color-accent)' : 'text-(--color-muted) hover:text-(--color-fg)',
              )}
            >
              {pm.id}
              {isActive ? (
                <span className="absolute inset-x-0 -bottom-px h-0.5 rounded-full bg-(--color-accent)" />
              ) : null}
            </button>
          )
        })}
      </div>
      <div className="mt-3 flex items-center justify-between gap-3 rounded-lg border border-(--color-border) bg-(--color-surface-1) px-4 py-3 font-mono text-sm">
        <span className="flex min-w-0 items-center gap-2">
          <span className="text-(--color-faint)">$</span>
          <span className="truncate text-(--color-fg)">{activeCmd}</span>
        </span>
        <CopyButton text={activeCmd} />
      </div>
    </div>
  )
}
