import type { PaneManifest } from './pane-manifest'

/**
 * Metadata about an installed extension (one pane package).
 */
export interface InstalledExtension {
  id: string
  name: string
  version: string
  description?: string
  author?: string
  /** Where it was installed from (e.g. "npm:zem-kanban-pane@1.0.0", "github:user/repo"). */
  source: string
  /** ISO 8601 timestamp of installation. */
  installedAt: string
  /** Pane kinds this extension provides renderers for. */
  panes: string[]
}

/**
 * Platform-agnostic storage backend for pane extensions.
 *
 * Tauri implements this with direct filesystem access (~/.zem/extensions/).
 * The web app implements this via server API calls, caching bundles in OPFS.
 */
export interface StorageBackend {
  /** List all installed extensions with their metadata. */
  listExtensions(): Promise<InstalledExtension[]>

  /** Get the full pane manifest for an extension. */
  getManifest(extensionId: string): Promise<PaneManifest>

  /** Get a file from an extension's directory as a blob URL. */
  getFile(extensionId: string, filePath: string): Promise<string>

  /** Install an extension from a source string (npm: or github:). */
  install(source: string): Promise<InstalledExtension>

  /** Uninstall an extension by its manifest id. */
  uninstall(extensionId: string): Promise<void>
}
