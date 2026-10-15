import type { PaneDescriptor } from './types'

class PaneRegistry {
  private panes = new Map<string, PaneDescriptor>()
  private listeners = new Set<() => void>()
  /** Server-declared pane kinds. Empty until the server catalog is synced. */
  private serverKinds = new Set<string>()

  register(descriptor: PaneDescriptor): void {
    // A frontend package may describe a pane, but it cannot make that kind
    // valid. Once the server catalog is known, only server-declared kinds may
    // enter the client registry.
    if (!this.serverKinds.has(descriptor.kind)) return
    this.panes.set(descriptor.kind, descriptor)
    this.emit()
  }

  /**
   * Apply the server's authoritative pane catalog. This also removes local
   * descriptors whose server addon is unavailable, so menus and renderers do
   * not advertise pane kinds the server cannot create or manage.
   */
  setServerKinds(kinds: readonly string[]): void {
    this.serverKinds = new Set(kinds)
    let changed = false
    for (const kind of this.panes.keys()) {
      if (!this.serverKinds.has(kind)) {
        this.panes.delete(kind)
        changed = true
      }
    }
    if (changed) this.emit()
  }

  hasServerKind(kind: string): boolean {
    return this.serverKinds.has(kind)
  }

  getServerKinds(): string[] | null {
    return Array.from(this.serverKinds)
  }

  unregister(kind: string): void {
    if (this.panes.delete(kind)) this.emit()
  }

  /** Notified whenever a pane is registered or unregistered. Panes install
   *  asynchronously, so consumers deriving state from the registry (keybindings,
   *  command palette) must re-read on change rather than snapshot once. */
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener)
    return () => { this.listeners.delete(listener) }
  }

  private emit(): void {
    for (const listener of this.listeners) listener()
  }

  get(kind: string): PaneDescriptor | undefined {
    return this.panes.get(kind)
  }

  getAll(): PaneDescriptor[] {
    return Array.from(this.panes.values())
  }

  getTitle(kind: string): string {
    return this.panes.get(kind)?.title ?? 'Pane'
  }

  getIcon(kind: string): PaneDescriptor['icon'] | undefined {
    return this.panes.get(kind)?.icon
  }

  get defaultKind(): string {
    const first = this.panes.keys().next()
    return first.done ? 'terminal' : first.value
  }
}

export const paneRegistry = new PaneRegistry()

// Expose globally so CDN-loaded pane bundles can access the same singleton instance
if (typeof window !== 'undefined') {
  (window as any).__zemPaneRegistry = paneRegistry
}
