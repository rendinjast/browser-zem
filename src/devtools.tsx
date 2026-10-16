import { useMemo, useState } from 'react'
import { X, Trash2 } from 'lucide-react'

export interface DevLogEntry {
  level: 'log' | 'info' | 'warn' | 'error' | 'debug'
  line: string
  at: number
  url: string
}

export interface NetworkEntry {
  method: string
  url: string
  status: number
  ms: number
  at: number
}

type Tab = 'console' | 'network' | 'errors'

const LEVEL_COLOR: Record<string, string> = {
  log: 'text-foreground/80',
  info: 'text-sky-400',
  warn: 'text-amber-400',
  error: 'text-red-400',
  debug: 'text-muted-foreground',
}

function timeStr(at: number): string {
  const d = new Date(at)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}:${String(d.getSeconds()).padStart(2, '0')}`
}

export function DevToolsDrawer({
  paneId,
  tagFor,
  logs,
  networkLogs,
  errorCount,
  onClear,
  onClose,
}: {
  paneId: number
  tagFor: (url: string, paneId: number) => string
  logs: DevLogEntry[]
  networkLogs: NetworkEntry[]
  errorCount: number
  onClear: () => void
  onClose: () => void
}) {
  const [tab, setTab] = useState<Tab>('console')
  const errors = useMemo(() => logs.filter(l => l.level === 'error'), [logs])

  const counts = {
    console: logs.length,
    network: networkLogs.length,
    errors: errorCount,
  }

  return (
    <div className="browser-devtools flex flex-col shrink-0 border-t border-border/60 bg-background/95 backdrop-blur-sm">
      {/* Header */}
      <div className="flex items-center gap-1 px-2 py-1 border-b border-border/60">
        {(['console', 'network', 'errors'] as Tab[]).map(t => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={`browser-devtab px-2 py-0.5 rounded text-[11px] uppercase tracking-wide ${tab === t ? 'bg-accent/15 text-accent' : 'text-muted-foreground hover:text-foreground'}`}
          >
            {t} <span className="opacity-60">({counts[t]})</span>
          </button>
        ))}
        <div className="flex-1" />
        <button className="browser-btn" onClick={onClear} title="Clear" aria-label="Clear">
          <Trash2 size={13} />
        </button>
        <button className="browser-btn" onClick={onClose} title="Close DevTools" aria-label="Close DevTools">
          <X size={14} />
        </button>
      </div>

      {/* Body */}
      <div className="browser-devtools-body overflow-auto font-mono text-[11px] leading-5">
        {tab === 'console' && (
          logs.length === 0 ? <EmptyState label="No console output yet." /> : (
            logs.map((l, i) => (
              <div key={`${l.at}-${i}`} className="browser-log-row flex gap-2 px-2 py-0.5 border-b border-border/30">
                <span className="text-muted-foreground/50 shrink-0">{timeStr(l.at)}</span>
                <span className={`shrink-0 uppercase w-11 ${LEVEL_COLOR[l.level]}`}>{l.level}</span>
                <span className="text-foreground/85 break-all whitespace-pre-wrap">{l.line}</span>
              </div>
            ))
          )
        )}

        {tab === 'network' && (
          networkLogs.length === 0 ? <EmptyState label="No network activity yet." /> : (
            <table className="w-full border-collapse">
              <tbody>
                {networkLogs.map((n, i) => {
                  const ok = n.status >= 200 && n.status < 400
                  return (
                    <tr key={`${n.at}-${i}`} className="browser-log-row border-b border-border/30">
                      <td className="px-2 py-0.5 text-muted-foreground/50 whitespace-nowrap">{timeStr(n.at)}</td>
                      <td className="px-2 py-0.5">
                        <span className={`px-1 rounded text-[10px] font-semibold ${ok ? 'bg-emerald-500/15 text-emerald-400' : n.status === 0 ? 'bg-red-500/15 text-red-400' : 'bg-amber-500/15 text-amber-400'}`}>
                          {n.status || 'ERR'}
                        </span>
                      </td>
                      <td className="px-2 py-0.5 text-sky-400 whitespace-nowrap">{n.method}</td>
                      <td className="px-2 py-0.5 text-foreground/75 break-all">{n.url}</td>
                      <td className="px-2 py-0.5 text-muted-foreground/60 whitespace-nowrap text-right">{n.ms}ms</td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          )
        )}

        {tab === 'errors' && (
          errors.length === 0 ? <EmptyState label="No errors." /> : (
            errors.map((l, i) => (
              <div key={`${l.at}-${i}`} className="browser-log-row flex flex-col gap-0.5 px-2 py-1 border-b border-border/30">
                <div className="flex gap-2 items-center">
                  <span className="text-red-400 shrink-0 uppercase w-11">error</span>
                  <span className="text-muted-foreground/50 text-[10px]">{timeStr(l.at)}</span>
                  <span className="text-muted-foreground/60 text-[10px]">{tagFor(l.url, paneId)}</span>
                </div>
                <div className="text-red-300/90 break-all whitespace-pre-wrap pl-[60px]">{l.line}</div>
              </div>
            ))
          )
        )}
      </div>
    </div>
  )
}

function EmptyState({ label }: { label: string }) {
  return <div className="px-3 py-4 text-muted-foreground/60 italic">{label}</div>
}
