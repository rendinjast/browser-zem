import type { ComponentType } from 'react'
import type { Binding, Scope } from '@zem/keybinding'

/**
 * The current app theme, pushed down to panes by the host. Updated whenever
 * the theme changes so panes can react (e.g. swap editor palettes, pick
 * syntax themes, or flip `color-scheme` for native controls).
 */
export interface PaneAppTheme {
  /** Resolved color scheme — always concrete, never `'system'`. */
  mode: 'dark' | 'light'
  /** Active theme variant id, e.g. `'default'`, `'catppuccin'`, `'dracula'`. */
  variant: string
}

export interface PaneComponentProps {
  paneId: number
  isFocused: boolean
  sessionId?: string
  params?: Record<string, unknown>
  /** Live app theme; re-renders the pane whenever it changes. */
  theme?: PaneAppTheme
}

export interface PaneContextMenuAction {
  label: string
  shortcut?: string
  action: () => void
}

/**
 * A keybinding a pane contributes to the keybinding manager. Panes return full
 * `Binding` objects so they can express keys, scope, group and a `when` guard.
 *
 * Two fields are stamped by the app, not the pane: the id becomes
 * `pane:<kind>:<id>` and `source` becomes `'pane'`. The `scope` defaults to
 * `'workspace'` — today every pane binding is an "open/create a pane of kind X"
 * command, which belongs to the workspace (where the pane exists), not the
 * focused pane.
 */
export type PaneBinding = Omit<Binding, 'id' | 'source' | 'group'> & {
  id?: string
  scope?: Scope
  /** Defaults to the pane's title. */
  group?: string
}

export interface PaneSettingsSelectField {
  type: 'select'
  key: string
  label: string
  description?: string
  value: string
  options: { value: string; label: string }[]
  optionsLoader?: () => Promise<{ value: string; label: string }[]>
  disabled?: boolean
  onChange: (value: string) => void
}

export interface PaneSettingsToggleField {
  type: 'toggle'
  key: string
  label: string
  description?: string
  value: boolean
  disabled?: boolean
  onChange: (value: boolean) => void
}

export interface PaneSettingsNumberField {
  type: 'number'
  key: string
  label: string
  description?: string
  value: number
  min: number
  max: number
  disabled?: boolean
  onChange: (value: number) => void
}

export interface PaneSettingsTextField {
  type: 'text'
  key: string
  label: string
  description?: string
  value: string
  placeholder?: string
  disabled?: boolean
  onChange: (value: string) => void
}

export interface PaneSettingsActionField {
  type: 'action'
  key: string
  label: string
  description?: string
  disabled?: boolean
  onAction: () => void
}

/** Read-only information row (e.g. version numbers in an "About" group). */
export interface PaneSettingsInfoField {
  type: 'info'
  key: string
  label: string
  value: string
  /** Render the value in a monospace font (version strings, hashes, sizes). */
  mono?: boolean
}

export type PaneSettingsField =
  | PaneSettingsSelectField
  | PaneSettingsToggleField
  | PaneSettingsNumberField
  | PaneSettingsTextField
  | PaneSettingsActionField
  | PaneSettingsInfoField

export interface PaneSettingsGroup {
  title: string
  fields: PaneSettingsField[]
}

export interface PaneSettingsSchema {
  groups: PaneSettingsGroup[]
  subscribe?: (listener: () => void) => () => void
  getSnapshot?: () => unknown
}

export type PaneLaunchField = PaneSettingsField

export interface PaneLaunchSchema {
  fields: PaneLaunchField[]
}

export interface PaneDescriptor {
  kind: string
  title: string
  /** Short human-readable summary shown in new-pane menus and the palette. */
  description?: string
  icon: ComponentType<{ className?: string }>
  /** Direct component reference (for built-in panes) or undefined when using lazy loading. */
  component?: ComponentType<PaneComponentProps>
  /** Lazy-loaded component from a renderer package (for installed extensions). */
  lazyComponent?: () => Promise<{ default: ComponentType<PaneComponentProps> }>
  settingsSchema?: PaneSettingsSchema
  launchSchema?: PaneLaunchSchema
  contextMenuActions?: (paneId: number, targetPath?: string) => PaneContextMenuAction[]
  /** Keybindings this pane contributes (typically "new <kind> pane"). The app
   *  stamps `id` (`pane:<kind>:<id>`), `source: 'pane'` and the `when` guard. */
  registerBindings?: () => PaneBinding[]
  onConfigChange?: (config: Record<string, unknown>) => void
  serialize?: (paneId: number) => Record<string, unknown> | undefined
  deserialize?: (paneId: number, data: Record<string, unknown>) => void
}
