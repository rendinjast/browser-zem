import type { ComponentType } from 'react'
import type { PaneComponentProps, PaneContextMenuAction, PaneSettingsSchema, PaneLaunchSchema, PaneBinding } from './types'

/**
 * Descriptor for a pane renderer — the client-side UI that renders a specific
 * pane kind. This is what pane packages provide. It is separate from
 * `PaneDescriptor` (which represents a server-declared pane kind).
 *
 * The `component` field is lazy-loaded so renderers are only fetched when the
 * user actually creates a pane of that kind.
 */
export interface PaneRendererDescriptor {
  /** Matches the server's pane kind string exactly. */
  kind: string
  /** Human-readable title shown in menus and tab headers. */
  title: string
  /** Icon reference string (e.g. "nerd:terminal", "emoji:📝"). */
  icon: string
  /** Short description shown in new-pane menus and the command palette. */
  description?: string
  /** Lazy-loaded React component. Returns a blob URL or module path. */
  component: () => Promise<{ default: ComponentType<PaneComponentProps> }>
  /** Optional CSS blob URL injected when the pane renders. */
  styleUrl?: string
  settingsSchema?: PaneSettingsSchema
  launchSchema?: PaneLaunchSchema
  contextMenuActions?: (paneId: number, targetPath?: string) => PaneContextMenuAction[]
  registerBindings?: () => PaneBinding[]
  onConfigChange?: (config: Record<string, unknown>) => void
  serialize?: (paneId: number) => Record<string, unknown> | undefined
  deserialize?: (paneId: number, data: Record<string, unknown>) => void
}

class PaneRendererRegistry {
  private renderers = new Map<string, PaneRendererDescriptor>()
  private listeners = new Set<() => void>()

  register(descriptor: PaneRendererDescriptor): void {
    this.renderers.set(descriptor.kind, descriptor)
    this.emit()
  }

  unregister(kind: string): void {
    if (this.renderers.delete(kind)) this.emit()
  }

  has(kind: string): boolean {
    return this.renderers.has(kind)
  }

  get(kind: string): PaneRendererDescriptor | undefined {
    return this.renderers.get(kind)
  }

  getAll(): PaneRendererDescriptor[] {
    return Array.from(this.renderers.values())
  }

  /** Notified whenever a renderer is registered or unregistered. */
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener)
    return () => { this.listeners.delete(listener) }
  }

  private emit(): void {
    for (const listener of this.listeners) listener()
  }
}

export const paneRendererRegistry = new PaneRendererRegistry()

// Expose globally so dynamically-loaded renderer bundles can self-register
if (typeof window !== 'undefined') {
  (window as any).__zemPaneRendererRegistry = paneRendererRegistry
}
