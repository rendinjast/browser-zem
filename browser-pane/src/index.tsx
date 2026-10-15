import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { type PaneComponentProps } from '@zem/pane'
import { cn } from './lib/utils'
import {
  ChevronRight, RotateCw, Home, Star, StarOff, Code2, ExternalLink,
} from 'lucide-react'
import { DevToolsDrawer, type DevLogEntry, type NetworkEntry } from './devtools'
import { BookmarksPanel, type Bookmark } from './bookmarks'
import { normalizeUrl, hostOf } from './utils'
import { Button } from './components/ui/button'

// Self-inject CSS so styles travel with the pane regardless of how it's loaded
import styles from './styles.css?inline'
if (typeof document !== 'undefined' && !document.querySelector('[data-pane-css="browser"]')) {
  const s = document.createElement('style'); s.textContent = styles; s.setAttribute('data-pane-css', 'browser'); document.head.appendChild(s)
}

// Access the shared paneRegistry via the global bridge
const paneRegistry = (window as any).__zemPaneRegistry

interface AddonApi {
  runPaneQuery: (paneKind: string, query: string, params?: string, paneId?: number) => Promise<unknown>
  runPaneMutation: (paneKind: string, mutation: string, params?: string, paneId?: number) => Promise<unknown>
}

let addonApi: AddonApi = {
  runPaneQuery: async () => ({}),
  runPaneMutation: async () => ({}),
}
export function setAddonApi(api: AddonApi) {
  addonApi = api
}

/** Workspace identity injected by the host (devtools tags + bookmarks). */
let workspaceId: number | null = null
let workspaceName = ''
export function setWorkspaceContext(id: number | null, name: string) {
  workspaceId = id
  workspaceName = name
}

// ─── State shape (mirrors the server's BrowserState) ────────────────────────

interface NavState {
  url: string
  history: string[]
  index: number
}

const EMPTY_STATE: NavState = { url: '', history: [], index: 0 }

function tagFor(url: string, paneId: number): string {
  const host = hostOf(url) || url || 'blank'
  return `[project: ${workspaceName || 'zem'}][url: ${host}][pane: ${paneId}]`
}

export function BrowserPane({ paneId, params }: PaneComponentProps) {
  const [port, setPort] = useState<number | null>(null)
  const [state, setState] = useState<NavState>(EMPTY_STATE)
  const [iframeSrc, setIframeSrc] = useState('')
  const [input, setInput] = useState('')
  const [loading, setLoading] = useState(false)
  const [title, setTitle] = useState('')
  const [pageUrl, setPageUrl] = useState('')       // current page's real URL (host-side)
  const [devToolsOpen, setDevToolsOpen] = useState(false)
  const [bookmarksOpen, setBookmarksOpen] = useState(false)
  const [bookmarks, setBookmarks] = useState<Bookmark[]>([])
  const [logs, setLogs] = useState<DevLogEntry[]>([])
  const [networkLogs, setNetworkLogs] = useState<NetworkEntry[]>([])
  const [errorCount, setErrorCount] = useState(0)
  const scrollTimer = useRef<number | null>(null)
  const iframeRef = useRef<HTMLIFrameElement | null>(null)
  const currentUrlRef = useRef('')

  // The initial URL may come from the launch dialog / "Open URL" action. Both
  // the `{launch: {url}}` (dialog) and `{url}` (action) shapes are accepted.
  const launchUrl = useMemo(() => {
    const p = params as { launch?: { url?: string } } | undefined
    const raw = (p as { url?: string } | undefined)?.url || p?.launch?.url
    return raw ? normalizeUrl(raw) : ''
  }, [params])

  const proxyBase = port ? `http://127.0.0.1:${port}/proxy` : ''

  // ── Bootstrap: discover proxy port + restore persisted state ─────────────
  useEffect(() => {
    let disposed = false
    let attempts = 0

    const tryInit = async (): Promise<void> => {
      attempts += 1
      try {
        const info = (await addonApi.runPaneQuery('browser', 'get_proxy_info', undefined, paneId)) as { port?: number }
        if (disposed) return
        if (info.port) setPort(info.port)
      } catch { /* retry below */ }
      if (disposed || attempts >= 20) return
      setTimeout(tryInit, 250)
    }
    tryInit()
    return () => { disposed = true }
  }, [paneId])

  useEffect(() => {
    if (!port) return
    let disposed = false
    ;(async () => {
      try {
        const s = (await addonApi.runPaneQuery('browser', 'get_state', undefined, paneId)) as NavState & { scroll?: number }
        if (disposed) return
        setState({ url: s.url ?? '', history: s.history ?? [], index: s.index ?? 0 })
        setInput(s.url ?? '')
        setPageUrl(s.url ?? '')
        if (s.url) {
          setIframeSrc(buildProxySrc(port, s.url, s.scroll ?? 0))
        } else if (launchUrl) {
          go(launchUrl)
        }
      } catch { /* server not ready yet */ }
    })()
    return () => { disposed = true }
  }, [port]) // eslint-disable-line react-hooks/exhaustive-deps

  // ── Bridge messages from the iframe ──────────────────────────────────────
  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      // Ignore messages from any other frame — with several browser panes open
      // each pane must only react to its own iframe's bridge events.
      if (iframeRef.current && e.source !== iframeRef.current.contentWindow) return
      const data = e.data as { __zem?: boolean; type?: string; data?: Record<string, unknown>; url?: string }
      if (!data?.__zem) return
      switch (data.type) {
        case 'ready': {
          setLoading(false)
          const u = typeof data.url === 'string' ? data.url : ''
          if (u) { setPageUrl(u); setInput(u) }
          break
        }
        case 'navigate': {
          // In-page link click or SPA history change: the bridge already
          // re-pointed the iframe; record the history entry (pushState → push,
          // replaceState → replace) and refresh the address bar.
          const u = typeof data.data?.url === 'string' ? normalizeUrl(data.data.url) : ''
          if (u) recordNavigation(u, data.data?.replace === true)
          break
        }
        case 'open-external': {
          const u = typeof data.data?.url === 'string' ? data.data.url : ''
          if (u) window.open(u, '_blank', 'noopener')
          break
        }
        case 'focus': {
          // The bridge lives inside the iframe and signals user interaction
          // so the host can focus this pane (iframe clicks never bubble to
          // the parent document).
          window.dispatchEvent(new CustomEvent('pane:focus', { detail: { paneId } }))
          break
        }
        case 'title': {
          const t = typeof data.data?.title === 'string' ? data.data.title : ''
          if (t) {
            setTitle(t)
            addonApi.runPaneMutation('browser', 'set_title', JSON.stringify({ title: t }), paneId).catch(() => { })
          }
          break
        }
        case 'scroll': {
          const y = typeof data.data?.y === 'number' ? data.data.y : 0
          if (scrollTimer.current) window.clearTimeout(scrollTimer.current)
          scrollTimer.current = window.setTimeout(() => {
            addonApi.runPaneMutation('browser', 'save_scroll', JSON.stringify({ y }), paneId).catch(() => { })
          }, 400)
          break
        }
        case 'console': {
          const rawLevel = String(data.data?.level ?? 'log')
          const level: DevLogEntry['level'] = (['log', 'info', 'warn', 'error', 'debug'] as const).includes(rawLevel as DevLogEntry['level'])
            ? rawLevel as DevLogEntry['level']
            : 'log'
          const args = Array.isArray(data.data?.args) ? (data.data.args as unknown[]) : []
          const line = args.map(a => typeof a === 'string' ? a : JSON.stringify(a)).join(' ')
          pushLog({ level, line, at: Date.now(), url: currentUrlRef.current })
          break
        }
        case 'error': {
          const msg = String(data.data?.message ?? '')
          const file = typeof data.data?.file === 'string' ? data.data.file : undefined
          const lineNum = typeof data.data?.line === 'number' ? data.data.line : undefined
          pushLog({ level: 'error', line: file ? `${msg} (${file}${lineNum ? `:${lineNum}` : ''})` : msg, at: Date.now(), url: currentUrlRef.current })
          break
        }
        case 'network': {
          const n: NetworkEntry = {
            method: String(data.data?.method ?? 'GET'),
            url: String(data.data?.url ?? ''),
            status: Number(data.data?.status ?? 0),
            ms: Number(data.data?.ms ?? 0),
            at: Date.now(),
          }
          setNetworkLogs(prev => [n, ...prev].slice(0, 200))
          break
        }
      }
    }
    window.addEventListener('message', onMessage)
    return () => {
      window.removeEventListener('message', onMessage)
      if (scrollTimer.current) window.clearTimeout(scrollTimer.current)
    }
  }, [paneId]) // eslint-disable-line react-hooks/exhaustive-deps

  currentUrlRef.current = pageUrl

  const pushLog = useCallback((entry: DevLogEntry) => {
    setLogs(prev => [entry, ...prev].slice(0, 300))
    if (entry.level === 'error') setErrorCount(c => c + 1)
  }, [])

  // ── Navigation helpers ───────────────────────────────────────────────────

  const buildProxySrc = (p: number, url: string, scrollY = 0) =>
    `${p > 0 ? `http://127.0.0.1:${p}/proxy` : ''}?url=${encodeURIComponent(url)}${scrollY > 0 ? `&scroll=${scrollY}` : ''}`

  const applyState = (s: NavState) => {
    setState(s)
    setInput(s.url)
    setPageUrl(s.url)
    if (s.url) setIframeSrc(buildProxySrc(port ?? 0, s.url))
  }

  /** User/URL-bar navigation: persists history, reloads the iframe. */
  const go = async (raw: string) => {
    const url = normalizeUrl(raw)
    if (!url) return
    setLoading(true)
    setIframeSrc(buildProxySrc(port ?? 0, url))
    setPageUrl(url)
    setInput(url)
    try {
      const s = (await addonApi.runPaneMutation('browser', 'navigate', JSON.stringify({ url }), paneId)) as NavState
      if (s?.history) setState(s)
    } catch { /* history persistence is best-effort */ }
  }

  /** In-page navigation reported by the bridge: update server history only. */
  const recordNavigation = async (url: string, replace: boolean) => {
    setPageUrl(url)
    setInput(url)
    try {
      const s = (await addonApi.runPaneMutation('browser', 'navigate', JSON.stringify({ url, replace }), paneId)) as NavState
      if (s?.history) setState(s)
    } catch { /* best-effort */ }
  }

  const goIndex = async (idx: number) => {
    if (idx < 0 || idx >= state.history.length) return
    setLoading(true)
    try {
      const s = (await addonApi.runPaneMutation('browser', 'navigate', JSON.stringify({ index: idx }), paneId)) as NavState
      if (s?.url) applyState(s)
    } catch { /* best-effort */ }
  }

  const reload = () => {
    if (!state.url) return
    setLoading(true)
    setIframeSrc(buildProxySrc(port ?? 0, state.url, 0) + `&t=${Date.now()}`)
  }

  const goHome = () => go('https://duckduckgo.com')

  // ── Bookmarks ────────────────────────────────────────────────────────────

  const refreshBookmarks = useCallback(async () => {
    try {
      const r = (await addonApi.runPaneQuery('browser', 'get_bookmarks', JSON.stringify({ workspace_id: workspaceId ?? 0 }), paneId)) as { bookmarks?: Bookmark[] }
      setBookmarks(r.bookmarks ?? [])
    } catch { /* best-effort */ }
  }, [paneId])

  useEffect(() => {
    if (bookmarksOpen) refreshBookmarks()
  }, [bookmarksOpen, refreshBookmarks])

  const isBookmarked = bookmarks.some(b => b.url === pageUrl)

  const toggleBookmark = async () => {
    const url = pageUrl
    if (!url) return
    if (isBookmarked) {
      await addonApi.runPaneMutation('browser', 'remove_bookmark', JSON.stringify({ workspace_id: workspaceId ?? 0, url }), paneId).catch(() => { })
    } else {
      await addonApi.runPaneMutation('browser', 'add_bookmark', JSON.stringify({ workspace_id: workspaceId ?? 0, url, title: title || hostOf(url) }), paneId).catch(() => { })
    }
    refreshBookmarks()
  }

  const openBookmark = (url: string) => {
    setBookmarksOpen(false)
    go(url)
  }

  const canBack = state.index > 0
  const canForward = state.index < state.history.length - 1

  return (
    <div className="browser-pane-root w-full h-full flex flex-col overflow-hidden text-foreground">
      {/* Toolbar (styled like the explorer pane) */}
      <div className="flex items-center gap-1 p-2 shrink-0 min-w-0 select-none border-b border-border/60">
        <Button onClick={() => goIndex(state.index - 1)} disabled={!canBack} variant="outline" size="icon-lg" title="Back" aria-label="Back">
          <ChevronRight className="size-4 rotate-180" />
        </Button>
        <Button onClick={() => goIndex(state.index + 1)} disabled={!canForward} variant="outline" size="icon-lg" title="Forward" aria-label="Forward">
          <ChevronRight className="w-4 h-4" />
        </Button>
        <Button onClick={goHome} variant="outline" size="icon-lg" title="Home" aria-label="Home">
          <Home className="w-4 h-4" />
        </Button>

        <form
          className="flex flex-1 min-w-0 mx-1"
          onSubmit={(e) => { e.preventDefault(); go(input) }}
        >
          <input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            placeholder="Enter URL…"
            spellCheck={false}
            autoComplete="off"
            className="flex-1 min-w-0 w-full text-xs p-2 rounded-lg border border-border focus:outline-none focus-visible:ring-2 focus-visible:ring-ring/30 dark:bg-input/20 bg-transparent font-mono"
          />
        </form>

        <Button onClick={reload} disabled={!state.url} variant="outline" size="icon-lg" title="Reload" aria-label="Reload">
          <RotateCw className={cn('w-4 h-4', loading && 'animate-spin')} />
        </Button>

        <Button onClick={toggleBookmark} disabled={!pageUrl} variant="outline" size="icon-lg" title={isBookmarked ? 'Remove bookmark' : 'Bookmark this page'} aria-label="Bookmark">
          {isBookmarked ? <Star className="size-4 text-amber-400" /> : <StarOff className="size-4" />}
        </Button>
        <Button onClick={() => setBookmarksOpen(o => !o)} variant="outline" size="icon-lg" title="Bookmarks" aria-label="Bookmarks">
          <Star className={cn('size-4', bookmarksOpen && 'text-accent')} />
        </Button>
        <Button onClick={() => { if (pageUrl) window.open(pageUrl, '_blank', 'noopener') }} variant="outline" size="icon-lg" title="Open in external browser" aria-label="Open externally">
          <ExternalLink className="size-4" />
        </Button>
        <Button
          onClick={() => setDevToolsOpen(o => !o)}
          variant="outline"
          size="icon-lg"
          className="relative"
          title="DevTools"
          aria-label="DevTools"
        >
          <Code2 className={cn('size-4', devToolsOpen && 'text-accent')} />
          {errorCount > 0 && !devToolsOpen && (
            <span className="browser-badge absolute -top-1 -right-1 min-w-3.5 h-3.5 px-0.5 rounded-full bg-red-500 text-white text-[9px] leading-3.5 text-center">
              {errorCount > 99 ? '99+' : errorCount}
            </span>
          )}
        </Button>
      </div>

      {/* Body */}
      <div className="relative flex-1 min-h-0">
        {proxyBase && state.url ? (
          <iframe
            key="browser-frame"
            ref={iframeRef}
            src={iframeSrc}
            className="absolute inset-0 w-full h-full border-0 bg-white"
            sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-modals allow-downloads"
            referrerPolicy="no-referrer"
            onLoad={() => setLoading(false)}
          />
        ) : (
          <div className="absolute inset-0 flex items-center justify-center">
            <div className="text-center space-y-2">
              <div className="text-3xl font-semibold text-muted-foreground/40">🌐</div>
              <p className="text-xs text-muted-foreground">
                {port ? 'Enter a URL above to start browsing' : 'Connecting to browser addon…'}
              </p>
            </div>
          </div>
        )}

        {/* Loading indicator */}
        {loading && (
          <div className="absolute top-0 left-0 right-0 h-0.5 bg-accent/20 overflow-hidden">
            <div className="browser-loading-bar h-full bg-accent" />
          </div>
        )}

        {/* Overlays */}
        {bookmarksOpen && (
          <div className="absolute inset-0 z-10 bg-background/95 backdrop-blur-sm">
            <BookmarksPanel
              bookmarks={bookmarks}
              currentUrl={pageUrl}
              onOpen={openBookmark}
              onRemove={(url) => {
                addonApi.runPaneMutation('browser', 'remove_bookmark', JSON.stringify({ workspace_id: workspaceId ?? 0, url }), paneId)
                  .then(refreshBookmarks).catch(() => { })
              }}
              onClose={() => setBookmarksOpen(false)}
            />
          </div>
        )}
      </div>

      {/* DevTools drawer */}
      {devToolsOpen && (
        <DevToolsDrawer
          paneId={paneId}
          tagFor={tagFor}
          logs={logs}
          networkLogs={networkLogs}
          errorCount={errorCount}
          onClear={() => { setLogs([]); setNetworkLogs([]); setErrorCount(0) }}
          onClose={() => setDevToolsOpen(false)}
        />
      )}

      {/* Status bar (styled like the explorer pane) */}
      <div className="shrink-0 p-2 font-medium text-muted-foreground flex items-center gap-2 border-t border-border">
        {!port ? (
          <span className="shrink-0 px-2 py-1 text-foreground/70 rounded-xl border border-border dark:bg-input/20">
            Connecting to addon…
          </span>
        ) : (
          <span className="shrink-0 px-2 py-1 text-foreground/70 rounded-xl border border-border dark:bg-input/20">
            {loading ? (
              <span className="flex items-center gap-1.5">
                <RotateCw className="w-3 h-3 animate-spin" /> Loading
              </span>
            ) : state.url ? 'Connected' : 'Idle'}
          </span>
        )}
        {state.url && (
          <span className="shrink-0 px-2 py-1 text-foreground/70 rounded-xl border border-border dark:bg-input/20 max-w-64 truncate">
            <span className="text-blue-400">{hostOf(pageUrl || state.url)}</span>
          </span>
        )}
        {title && (
          <span className="shrink-0 px-2 py-1 text-foreground/70 rounded-xl border border-border dark:bg-input/20 max-w-48 truncate">
            {title}
          </span>
        )}
        <div className="flex-1" />
        {errorCount > 0 && (
          <span className="shrink-0 px-2 py-1 text-foreground/70 rounded-xl border border-border dark:bg-input/20">
            <span className="text-red-400">{errorCount}</span> errors
          </span>
        )}
        {bookmarks.length > 0 && (
          <span className="shrink-0 px-2 py-1 text-foreground/70 rounded-xl border border-border dark:bg-input/20">
            <span className="text-amber-400">{bookmarks.length}</span> bookmarks
          </span>
        )}
      </div>
    </div>
  )
}

function GlobeIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6">
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3a15 15 0 0 1 0 18M12 3a15 15 0 0 0 0 18" />
    </svg>
  )
}

export const browserLaunchSchema = {
  fields: [{
    type: 'text' as const,
    key: 'url',
    label: 'URL',
    description: 'Page to open when the pane starts (e.g. https://example.com)',
    value: '',
    placeholder: 'https://example.com',
    onChange: () => {},
  }],
}

export function register(): void {
  paneRegistry.register({
    kind: 'browser',
    title: 'Browser',
    description: 'Webview pane with history, bookmarks and devtools',
    icon: GlobeIcon,
    component: BrowserPane,
    launchSchema: browserLaunchSchema,
    contextMenuActions: (paneId: number) => [
      { label: 'Open DevTools', action: () => window.dispatchEvent(new CustomEvent('pane-action', { detail: { paneId, action: 'devtools' } })) },
      { label: 'Reload', action: () => window.dispatchEvent(new CustomEvent('pane-action', { detail: { paneId, action: 'reload' } })) },
    ],
    registerBindings() {
      return [{
        id: 'new-browser',
        keys: [{ key: 'b', meta: true, shift: true }],
        description: 'New Browser pane',
        action: () => window.dispatchEvent(new CustomEvent('pane:new-tile', { detail: { kind: 'browser' } })),
      }]
    },
  })
}

export function unregister(): void {
  paneRegistry.unregister('browser')
}
export const registerBrowserPane = register
