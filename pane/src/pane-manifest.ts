/**
 * Types for the pane-manifest.toml file that ships with every web pane package.
 *
 * This mirrors the server's addon manifest.toml structure but is focused on
 * client-side UI rendering — no binary, no operations (those are server-side).
 *
 * Example pane-manifest.toml:
 * ```toml
 * id          = "com.example.kanban-pane"
 * name        = "kanban-pane"
 * version     = "1.0.0"
 * description = "Kanban board pane renderer for Zem"
 *
 * [[pane]]
 * kind        = "kanban"
 * title       = "Kanban Board"
 * icon        = "nerd:board"
 * entry       = "./pane.js"
 * style       = "./pane.css"
 * ```
 */

export interface PaneManifestPane {
  /** Matches the server's pane kind exactly. */
  kind: string
  /** Human-readable title for menus and tab headers. */
  title: string
  /** Icon reference (e.g. "nerd:terminal", "emoji:📝"). */
  icon: string
  /** Short description for tooltips and the marketplace. */
  description?: string
  /** Path to the ES module bundle, relative to the manifest. */
  entry: string
  /** Optional path to a CSS file, relative to the manifest. */
  style?: string
}

export interface PaneManifest {
  /** Reverse-domain identifier (e.g. "com.example.kanban-pane"). */
  id: string
  /** Short machine name (e.g. "kanban-pane"). */
  name: string
  /** Semver version. */
  version: string
  description?: string
  author?: string
  homepage?: string
  license?: string
  /** One package can render multiple pane kinds. */
  pane: PaneManifestPane[]
}

/**
 * Parse a TOML string into a PaneManifest.
 * Uses a minimal inline TOML parser to avoid a dependency.
 */
export function parseManifest(toml: string): PaneManifest {
  const lines = toml.split('\n')
  const root: Record<string, unknown> = {}
  const panes: Record<string, unknown>[] = []
  let currentSection: 'root' | 'pane' = 'root'
  let currentPane: Record<string, unknown> | null = null

  for (const line of lines) {
    const trimmed = line.trim()

    // Skip empty lines and comments
    if (trimmed === '' || trimmed.startsWith('#')) continue

    // Section header
    const sectionMatch = trimmed.match(/^\[\[pane\]\]$/)
    if (sectionMatch) {
      currentSection = 'pane'
      if (currentPane) panes.push(currentPane)
      currentPane = {}
      continue
    }

    // Key = value
    const kvMatch = trimmed.match(/^(\w+)\s*=\s*(.+)$/)
    if (!kvMatch) continue

    const [, key, rawValue] = kvMatch

    // Parse the value
    let value: unknown = rawValue.trim()

    // Remove surrounding quotes
    if ((value as string).startsWith('"') && (value as string).endsWith('"')) {
      value = (value as string).slice(1, -1)
    } else if ((value as string).startsWith("'") && (value as string).endsWith("'")) {
      value = (value as string).slice(1, -1)
    } else if ((value as string) === 'true') {
      value = true
    } else if ((value as string) === 'false') {
      value = false
    }

    if (currentSection === 'pane' && currentPane) {
      currentPane[key] = value
    } else {
      root[key] = value
    }
  }

  if (currentPane) panes.push(currentPane)

  return {
    id: root.id as string,
    name: root.name as string,
    version: root.version as string,
    description: root.description as string | undefined,
    author: root.author as string | undefined,
    homepage: root.homepage as string | undefined,
    license: root.license as string | undefined,
    pane: panes.map(p => ({
      kind: p.kind as string,
      title: p.title as string,
      icon: p.icon as string,
      description: p.description as string | undefined,
      entry: p.entry as string,
      style: p.style as string | undefined,
    })),
  }
}
