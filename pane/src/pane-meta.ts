type PaneMeta = Record<string, unknown>

/**
 * Per-pane metadata shared between panes and the host. Pane display names and
 * icons are resolved server-side and never pushed here; the registry carries
 * pane-internal live metadata only (e.g. the terminal pane's current cwd for
 * its own footer).
 */
class PaneMetaRegistry {
  private meta = new Map<number, PaneMeta>()
  private listeners = new Set<(paneId: number) => void>()

  set(paneId: number, data: PaneMeta): void {
    const existing = this.meta.get(paneId) ?? {}
    this.meta.set(paneId, { ...existing, ...data })
    this.emit(paneId)
  }

  get(paneId: number): PaneMeta | undefined {
    return this.meta.get(paneId)
  }

  delete(paneId: number): void {
    if (this.meta.delete(paneId)) this.emit(paneId)
  }

  /** Notified whenever a specific pane's meta changes. Returns an unsubscribe fn. */
  subscribe(paneId: number, listener: () => void): () => void {
    const wrapped = (changed: number) => {
      if (changed === paneId) listener()
    }
    this.listeners.add(wrapped)
    return () => {
      this.listeners.delete(wrapped)
    }
  }

  private emit(paneId: number): void {
    for (const listener of [...this.listeners]) listener(paneId)
  }
}

export const paneMetaRegistry = new PaneMetaRegistry()

// Expose globally so CDN-loaded pane bundles can access the same singleton instance
if (typeof window !== 'undefined') {
  (window as any).__zemPaneMetaRegistry = paneMetaRegistry
}
