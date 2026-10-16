import { X, Trash2, ExternalLink } from 'lucide-react'
import { hostOf } from './utils'

export interface Bookmark {
  url: string
  title?: string
}

export function BookmarksPanel({
  bookmarks,
  currentUrl,
  onOpen,
  onRemove,
  onClose,
}: {
  bookmarks: Bookmark[]
  currentUrl: string
  onOpen: (url: string) => void
  onRemove: (url: string) => void
  onClose: () => void
}) {
  return (
    <div className="w-full h-full flex flex-col">
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border/60">
        <h3 className="text-xs font-semibold">Bookmarks</h3>
        <span className="text-[11px] text-muted-foreground">({bookmarks.length})</span>
        <div className="flex-1" />
        <button className="browser-btn" onClick={onClose} title="Close" aria-label="Close">
          <X size={14} />
        </button>
      </div>

      <div className="flex-1 overflow-auto">
        {bookmarks.length === 0 ? (
          <div className="px-3 py-6 text-center text-xs text-muted-foreground/70 italic">
            No bookmarks in this workspace yet.<br />
            Use the star in the toolbar to save the current page.
          </div>
        ) : (
          bookmarks.map(b => {
            const active = b.url === currentUrl
            return (
              <div
                key={b.url}
                className={`group flex items-center gap-2 px-3 py-1.5 border-b border-border/30 hover:bg-accent/10 cursor-pointer ${active ? 'bg-accent/5' : ''}`}
                onClick={() => onOpen(b.url)}
              >
                <span className="text-[10px] font-mono uppercase text-muted-foreground/60 shrink-0 w-6">▸</span>
                <div className="flex-1 min-w-0">
                  <div className="text-xs truncate">{b.title || hostOf(b.url)}</div>
                  <div className="text-[10px] font-mono text-muted-foreground truncate">{b.url}</div>
                </div>
                {active && <span className="text-[9px] uppercase text-accent shrink-0">current</span>}
                <button
                  className="browser-btn opacity-0 group-hover:opacity-100 shrink-0"
                  title="Open in external browser"
                  onClick={(e) => { e.stopPropagation(); window.open(b.url, '_blank', 'noopener') }}
                >
                  <ExternalLink size={12} />
                </button>
                <button
                  className="browser-btn opacity-0 group-hover:opacity-100 shrink-0 hover:text-red-400"
                  title="Remove bookmark"
                  onClick={(e) => { e.stopPropagation(); onRemove(b.url) }}
                >
                  <Trash2 size={12} />
                </button>
              </div>
            )
          })
        )}
      </div>
    </div>
  )
}
