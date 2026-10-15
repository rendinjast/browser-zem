import type { Binding, ConflictRecord, KeySeq, RegistrySnapshot, Scope } from "./types"
import { scopeRank } from "./types"
import { seqKey, seqEquals, canonicalizeSeq } from "./keystroke"
import { ScopeTrie } from "./trie"

type Listener = (snap: RegistrySnapshot) => void

/**
 * The central store for all keybindings. Owns one {@link ScopeTrie} per scope.
 * Supports dynamic register / unregister / update / toggle at runtime.
 *
 * Resolves bindings by walking scopes in priority order. Conflict detection
 * fires within a scope: two bindings with the same KeySeq in the same scope
 * collide (one shadows the other deterministically by registration order).
 *
 * User overrides live in a separate map and are applied at key time: a binding's
 * `keys` stays the pristine default, the trie is keyed by the override when one
 * exists. Resetting is dropping the override; re-registering (pane/addon
 * hot-reload) transparently re-applies it.
 */
export class KeybindingRegistry {
  private tries: Map<Scope, ScopeTrie> = new Map()
  private listeners: Set<Listener> = new Set()
  private conflicts: ConflictRecord[] = []
  private cachedSnapshot: RegistrySnapshot | null = null
  /** User overrides: binding id -> canonical KeySeq. */
  private overrides = new Map<string, KeySeq>()
  /** Default keys as declared, keyed by binding id (for reset + display). */
  private defaults = new Map<string, KeySeq>()

  constructor() {
    for (const s of ["zemx", "global", "workspace", "pane", "local"] as Scope[]) {
      this.tries.set(s, new ScopeTrie(s))
    }
  }

  // ── Mutations ──────────────────────────────────────────────────────────────
  register(binding: Binding): boolean {
    // Canonicalize once at the entry point so the trie is keyed by canonical
    // chords regardless of how a binding was declared ("cmd+shift+[" and a
    // KeyboardEvent ⌘{ both land on { key: '{', meta: true }).
    const declared = canonicalizeSeq(binding.keys)
    this.defaults.set(binding.id, declared)
    // The trie is keyed by the *effective* sequence so a rebound binding is
    // reachable by the keys the user chose, not the ones it shipped with.
    const b: Binding = { ...binding, keys: this.overrides.get(binding.id) ?? declared }
    // A binding may move between scopes on re-register; drop any stale entry
    // in the other tries first so it can't be resolved twice.
    for (const [s, trie] of this.tries) {
      if (s !== b.scope) trie.unregister(b.id)
    }
    this.tries.get(b.scope)!.register(b)
    // cleanConflicts wipes every record touching this id, then recomputes them
    // against the trie's current contents — no separate pre-scan needed.
    this.cleanConflicts(b.id)
    this.emit()
    return true
  }

  unregister(id: string): boolean {
    for (const trie of this.tries.values()) {
      if (trie.unregister(id)) {
        // The override outlives the binding on purpose: a pane that unregisters
        // on uninstall and registers again on reinstall keeps the user's key.
        this.defaults.delete(id)
        this.cleanConflicts(id)
        this.emit()
        return true
      }
    }
    return false
  }

  /** Remove every binding whose id starts with prefix. Handy for addons:
   *  `unregisterSource('addon:stats-pane')`. */
  unregisterByPrefix(prefix: string): number {
    let n = 0
    const toRemove: string[] = []
    for (const trie of this.tries.values()) {
      for (const b of trie.entries()) {
        if (b.id.startsWith(prefix)) toRemove.push(b.id)
      }
    }
    for (const id of toRemove) {
      if (this.unregister(id)) n++
    }
    return n
  }

  update(id: string, patch: Partial<Binding>): boolean {
    const found = this.get(id)
    if (!found) return false
    // `found.keys` is the *effective* sequence, so re-registering it verbatim
    // would promote a user override into the declared default and make reset a
    // no-op. Restore the declared keys unless the patch names new ones.
    const keys = patch.keys ?? this.defaults.get(id) ?? found.keys
    // `scope` is patchable (a pane binding can be promoted to workspace);
    // `id` is the identity and stays fixed.
    return this.register({ ...found, ...patch, keys, id: found.id })
  }

  toggle(id: string, enabled?: boolean): boolean {
    const found = this.get(id)
    if (!found) return false
    // `enabled` defaults to true when unset, so an untouched binding toggles off.
    return this.update(id, { enabled: enabled ?? found.enabled === false })
  }

  clearScope(scope: Scope): void {
    this.tries.get(scope)!.clear()
    this.conflicts = this.conflicts.filter((c) => c.scope !== scope)
    this.emit()
  }

  // ── User overrides ─────────────────────────────────────────────────────────
  // An override is stored per binding id and re-keys that binding in its trie.
  // It is kept even when no binding currently claims the id, so overrides for a
  // pane that is temporarily uninstalled are not silently lost.

  /** Rebind one binding. Pass the sequence the user recorded. */
  setOverride(id: string, keys: KeySeq): boolean {
    const canonical = canonicalizeSeq(keys)
    if (canonical.length === 0) return false
    const existing = this.overrides.get(id)
    if (existing && seqEquals(existing, canonical)) return true
    this.overrides.set(id, canonical)
    return this.rekey(id)
  }

  /** Drop a user override, restoring the declared default. */
  clearOverride(id: string): boolean {
    if (!this.overrides.delete(id)) return false
    return this.rekey(id)
  }

  /** Drop every user override. */
  clearAllOverrides(): void {
    if (this.overrides.size === 0) return
    const ids = [...this.overrides.keys()]
    this.overrides.clear()
    for (const id of ids) this.applyKeys(id)
    this.emit()
  }

  /**
   * Replace the whole override set in one pass — the shape config load hands us.
   * Bindings that gain, lose, or change an override are re-keyed; the rest are
   * untouched.
   */
  setOverrides(next: Record<string, KeySeq>): void {
    const canonical = new Map<string, KeySeq>()
    for (const [id, keys] of Object.entries(next)) {
      const seq = canonicalizeSeq(keys)
      if (seq.length > 0) canonical.set(id, seq)
    }
    const touched = new Set([...this.overrides.keys(), ...canonical.keys()])
    this.overrides = canonical
    for (const id of touched) {
      const before = this.get(id)
      if (!before) continue
      const effective = canonical.get(id) ?? this.defaults.get(id)
      if (effective && !seqEquals(before.keys, effective)) this.applyKeys(id)
    }
    // Emit unconditionally: overrides for a not-yet-registered binding change no
    // trie but still change what the settings UI must render.
    this.emit()
  }

  /** The sequence a binding was declared with, before any user override. */
  defaultKeys(id: string): KeySeq | undefined {
    return this.defaults.get(id)
  }

  override(id: string): KeySeq | undefined {
    return this.overrides.get(id)
  }

  /** Re-insert a binding under its current effective keys, then notify. */
  private rekey(id: string): boolean {
    this.applyKeys(id)
    this.emit()
    return true
  }

  /** Re-key one binding without notifying — callers batch the emit. */
  private applyKeys(id: string): void {
    const found = this.get(id)
    // No binding registered under this id (yet) — the override is recorded and
    // will be picked up when it registers.
    if (!found) return
    const effective = this.overrides.get(id) ?? this.defaults.get(id) ?? found.keys
    this.tries.get(found.scope)!.register({ ...found, keys: effective })
    this.cleanConflicts(id)
  }

  // ── Lookup / resolve helpers ───────────────────────────────────────────────

  get(id: string): Binding | undefined {
    for (const trie of this.tries.values()) {
      const b = trie.get(id)
      if (b) return b
    }
    return undefined
  }

  scope(scope: Scope): ScopeTrie {
    return this.tries.get(scope)!
  }

  all(): Binding[] {
    const out: Binding[] = []
    for (const trie of this.tries.values()) {
      for (const b of trie.entries()) out.push(b)
    }
    return out
  }

  groups(): { id: string; label: string; bindings: { binding: Binding; tags: string[] }[] }[] {
    // Used by the help/settings renderers. Stable grouping by `group`.
    const map = new Map<string, Binding[]>()
    for (const b of this.all()) {
      const key = b.group || "Misc"
      let bucket = map.get(key)
      if (!bucket) map.set(key, (bucket = []))
      bucket.push(b)
    }
    return Array.from(map.entries()).map(([label, bindings]) => ({
      id: label,
      label,
      // Copy before sorting: `bindings` may be shared with a cached snapshot.
      bindings: [...bindings]
        .sort((a, z) => seqKey(a.keys).localeCompare(seqKey(z.keys)))
        .map((binding) => ({
          binding,
          tags: [binding.scope, binding.source ?? "builtin"],
        })),
    }))
  }

  // ── Conflict bookkeeping ────────────────────────────────────────────────────
  private recordConflict(id: string, clashesWith: string, scope: Scope, keys: KeySeq) {
    const dup =
      this.conflicts.find((c) => c.bindingId === id && c.clashesWith === clashesWith) ??
      this.conflicts.find((c) => c.bindingId === clashesWith && c.clashesWith === id)
    if (dup) return
    this.conflicts.push({ bindingId: id, clashesWith, scope, keys })
  }

  private cleanConflicts(id: string) {
    this.conflicts = this.conflicts.filter(
      (c) => c.bindingId !== id && c.clashesWith !== id,
    )
    // Recompute conflicts for this id conservatively
    const b = this.get(id)
    if (b) {
      for (const other of this.scope(b.scope).entries()) {
        if (other.id === b.id) continue
        if (seqEquals(other.keys, b.keys)) this.recordConflict(b.id, other.id, b.scope, b.keys)
      }
    }
  }

  // ── Subscribe ──────────────────────────────────────────────────────────────
  subscribe(fn: Listener): () => void {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  snapshot(): RegistrySnapshot {
    // Cached: useSyncExternalStore compares snapshots by identity and would
    // re-render forever if this allocated on every call. Invalidated in emit().
    this.cachedSnapshot ??= {
      bindings: this.all(),
      conflicts: this.conflicts,
      overrides: Object.fromEntries(this.overrides),
      defaults: Object.fromEntries(this.defaults),
    }
    return this.cachedSnapshot
  }

  private emit() {
    this.cachedSnapshot = null
    const snap = this.snapshot()
    for (const fn of this.listeners) fn(snap)
  }
}

// ── Scope-intersection priority used by the engine ────────────────────────────

/** Sort scopes so the ones with highest priority come first. */
export function orderedScopes(active: Scope[]): Scope[] {
  return [...active].sort((a, b) => scopeRank(a) - scopeRank(b))
}
