// ─── Core public types for the keybinding manager ──────────────────────────────
// A "scope" decides which bindings are eligible during a given keydown. Scopes are
// consulted in priority order (see SCOPE_PRIORITY). Higher-priority scopes swallow
// the event before lower-priority ones get a chance.
export type Scope = "zemx" | "local" | "pane" | "workspace" | "global"

export const SCOPE_PRIORITY: Scope[] = ["zemx", "local", "pane", "workspace", "global"]
// NOTE: "zemx" is a sticky focus mode (entered via <leader> a) whose single-key
// bindings must swallow keys even inside inputs. "local" (text/explorer) is next
// so input handlers win over app-wide shortcuts. "global" is lowest — it only
// fires if no inner scope claimed the key. To opt a binding into always firing
// regardless of scope, set `force`.

export function scopeRank(scope: Scope): number {
  return SCOPE_PRIORITY.indexOf(scope)
}

// A single normalized chord. `key` is always lower-case and uses KeyboardEvent.key
// semantics (e.g. 's', 'escape', 'enter', ' ', 'arrowup').
export interface KeyChord {
  key: string
  ctrl?: boolean
  meta?: boolean // ⌘ on mac / super on linux
  alt?: boolean
  shift?: boolean
}

// A binding is an ordered sequence of chords:
//   length 1    => single stroke        e.g. [{ meta: true, key: 'o' }]
//   length > 1  => chord/leader sequence e.g. [{ key: ' ' }, { key: 's' }] = "<leader> s"
export type KeySeq = KeyChord[]

export interface BindingContext {
  event: KeyboardEvent
  scope: Scope
  focusedPaneId: number | null
  // the active scope stack at dispatch time (highest priority first)
  activeScopes: Scope[]
}

export type BindingSource = "builtin" | "pane" | "addon" | "user"

export interface Binding {
  /** Stable unique id, e.g. 'global:toggle-overview'. */
  id: string
  scope: Scope
  /** Ordered sequence of chords. */
  keys: KeySeq
  description: string
  group: string
  action: (ctx: BindingContext) => void
  /** Dynamic enable/disable predicate evaluated at dispatch time. */
  when?: (ctx: BindingContext) => boolean
  enabled?: boolean
  source?: BindingSource
  /**
   * Bypass scope priority and always be eligible (e.g. global "close dialog"
   * Escape that must work even when a text field is focused). Use sparingly.
   */
  force?: boolean
}

export interface ConflictRecord {
  bindingId: string
  scope: Scope
  keys: KeySeq
  /** Existing binding id/keys this one clashes with. */
  clashesWith: string
}

export interface RegistrySnapshot {
  /** Every registered binding, with `keys` already resolved to the effective
   *  sequence (user override applied on top of the declared default). */
  bindings: Binding[]
  conflicts: ConflictRecord[]
  /** binding id -> user override. Presence means "the user rebound this". */
  overrides: Record<string, KeySeq>
  /** binding id -> the sequence the binding was declared with. */
  defaults: Record<string, KeySeq>
}

/** Result of resolving a typing buffer against the trie. */
export type ResolveResult =
  | { status: "match"; binding: Binding; scope: Scope }
  | { status: "partial" }
  | { status: "none"; reason: "no-binding" | "disabled" | "when-false" | "scope-inactive" }

export interface LeaderConfig {
  /** The chord(s) that act as a leader prefix. Multiple allowed. */
  keys: KeyChord[]
}
