import type {
  Binding, BindingContext, KeyChord, ResolveResult, Scope, LeaderConfig,
} from "./types"
import { KeybindingRegistry, orderedScopes } from "./registry"
import { ScopeTrie } from "./trie"
import { eventToChord as normalizeEvent, isStandaloneModifier, chordKey } from "./keystroke"
import { resolveLeaderConfig, isLeaderChord } from "./leader"

// ── Engine: single source of truth for dispatching key events ──────────────────

export interface EngineOptions {
  leader?: Partial<LeaderConfig>
  /** When true, do not actually attach a window listener (test mode). */
  passive?: boolean
}

export class KeybindingEngine {
  readonly registry: KeybindingRegistry
  private leader: LeaderConfig
  private activeScopes: Scope[] = ["global"]
  private attached = false
  private suspended = false
  private buffer: KeyChord[] = []
  private opts: EngineOptions
  private bufferListeners: Set<(buffer: KeyChord[]) => void> = new Set()

  constructor(registry: KeybindingRegistry, opts: EngineOptions = {}) {
    this.registry = registry
    this.opts = opts
    this.leader = resolveLeaderConfig(opts.leader)
  }

  // ── Scope stack management ─────────────────────────────────────────────────
  /** Replace the active scope stack. Highest priority first is enforced. */
  setActiveScopes(scopes: Scope[]) {
    const set = new Set<Scope>(scopes)
    // 'global' is always implicitly active
    set.add("global")
    this.activeScopes = orderedScopes(Array.from(set))
    this.notifyActiveScopes()
  }

  pushScope(scope: Scope) {
    if (!this.activeScopes.includes(scope)) {
      this.activeScopes = orderedScopes([...this.activeScopes, scope])
      this.notifyActiveScopes()
    }
  }

  popScope(scope: Scope) {
    if (scope === "global") return
    this.activeScopes = orderedScopes(this.activeScopes.filter((s) => s !== scope))
    this.notifyActiveScopes()
  }

  /** True when a scope is currently part of the active stack. */
  isScopeActive(scope: Scope): boolean {
    return this.activeScopes.includes(scope)
  }

  /** Exit transient keybinding modes without affecting the focused pane. */
  exitTransientModes(): void {
    this.resetBuffer()
    if (this.activeScopes.includes('zemx')) this.popScope('zemx')
    for (const listener of this.transientExitListeners) listener()
  }

  /** Subscribe to transient-mode exits so UI-only feedback can be dismissed too. */
  subscribeTransientExit(listener: () => void): () => void {
    this.transientExitListeners.add(listener)
    return () => { this.transientExitListeners.delete(listener) }
  }

  private transientExitListeners: Set<() => void> = new Set()

  /** Subscribe to changes in the active scope stack (e.g. for a ZEMX badge). */
  subscribeActiveScopes(fn: (scopes: Scope[]) => void): () => void {
    this.scopeListeners.add(fn)
    return () => { this.scopeListeners.delete(fn) }
  }

  private scopeListeners: Set<(scopes: Scope[]) => void> = new Set()

  private notifyActiveScopes() {
    if (this.scopeListeners.size === 0) return
    const copy = [...this.activeScopes]
    for (const fn of this.scopeListeners) fn(copy)
  }

  // ── Leader configuration ───────────────────────────────────────────────────
  setLeader(cfg: Partial<LeaderConfig>) {
    this.leader = resolveLeaderConfig(cfg)
  }

  // ── Attaching ───────────────────────────────────────────────────────────────
  attach(win: Window & typeof globalThis = window): () => void {
    if (this.attached || this.opts.passive) return () => {}
    this.attached = true
    const onKey = (e: KeyboardEvent) => this.handle(e)
    win.addEventListener("keydown", onKey, true)
    return () => {
      win.removeEventListener("keydown", onKey, true)
      this.attached = false
    }
  }

  // ── Core dispatch ───────────────────────────────────────────────────────────
  /**
   * Public (also used in tests). Processes one KeyboardEvent. Returns the
   * resolution status so callers/tests can assert.
   */
  handle(e: KeyboardEvent): ResolveResult {
    // The command palette uses a synthetic ArrowUp/ArrowDown event to delegate
    // Ctrl+J/Ctrl+K navigation to Base UI's native highlight manager. Do not
    // route that synthetic event back through the global registry.
    if ((e as KeyboardEvent & { __zemPaletteNavigation?: boolean }).__zemPaletteNavigation) {
      return { status: "none", reason: "disabled" }
    }
    if (this.suspended) return { status: "none", reason: "disabled" }
    if (e.defaultPrevented) return { status: "none", reason: "disabled" }
    if (isStandaloneModifier(e)) return { status: "none", reason: "disabled" }

    const chord: KeyChord = this.chordFromEvent(e)
    const candidateScopes: Scope[] = this.activeScopes

    // Escape exits ZEMX (sticky focus) mode even when no sequence is pending.
    if (chord.key === "escape" && this.activeScopes.includes("zemx")) {
      this.popScope("zemx")
      this.resetBuffer()
      e.preventDefault()
      return { status: "none", reason: "no-binding" }
    }

    // When the focus is on an editable element (input, textarea,
    // contenteditable), plain typing keys pass through to the element instead
    // of triggering bindings. Modified chords (Cmd+S), the leader key, escape,
    // and in-progress leader sequences still dispatch normally. ZEMX (sticky
    // focus mode) is the exception: its single-key bindings must swallow keys
    // even inside inputs, so the editable pass-through is disabled while the
    // `zemx` scope is active.
    if (
      this.buffer.length === 0 &&
      !this.activeScopes.includes("zemx") &&
      this.targetIsEditable(e) &&
      !chord.ctrl &&
      !chord.meta &&
      !chord.alt &&
      chord.key !== "escape" &&
      !isLeaderChord(chord, this.leader)
    ) {
      return { status: "none", reason: "disabled" }
    }

    // Leader sequences always work — even inside editable targets (terminal,
    // textarea, input). The leader key takes priority over all other handlers.
    if (this.buffer.length > 0) {
      // Escape cancels the pending sequence (exits ZEM/ZEMX mode) without
      // firing an "unknown sequence" flash.
      if (chord.key === "escape") {
        this.resetBuffer()
        e.preventDefault()
        return { status: "none", reason: "no-binding" }
      }
      // Re-pressing the leader descends into the deeper (ZEMX) layer. If any
      // binding is registered as `<leader> <leader> …`, the doubled leader stays
      // in the buffer so that sequence can resolve; otherwise the extra leader
      // just re-arms ZEM mode instead of aborting it.
      if (isLeaderChord(chord, this.leader)) {
        const deeper: KeyChord[] = [...this.buffer, chord]
        const r = this.resolveInScopes(candidateScopes, deeper, e)
        if (r.status === "match") {
          return this.dispatchResult(e, r, chord)
        }
        if (r.status === "partial") {
          this.buffer = deeper
          this.notifyBuffer()
          e.preventDefault()
          return r
        }
        this.buffer = [chord]
        this.notifyBuffer()
        e.preventDefault()
        return { status: "partial" }
      }
      this.buffer.push(chord)
      this.notifyBuffer()
      const r = this.resolveInScopes(candidateScopes, this.buffer, e)
      return this.dispatchResult(e, r, chord)
    }

    // Single-chord attempt. An exact match wins immediately; a strict prefix
    // opens a sequence buffer — this covers both the leader (`<leader> s`) and
    // vim-style multi-key sequences (`gg`) that don't start with the leader.
    const attempt: KeyChord[] = [chord]
    const r = this.resolveInScopes(candidateScopes, attempt, e)
    if (r.status === "match") {
      return this.dispatchResult(e, r, chord)
    }
    if (r.status === "partial") {
      // A partial match that is neither the leader nor the start of a
      // non-leader multi-chord sequence (e.g. vim `gg`) is a stale leader
      // chord — do NOT start a sequence buffer for it.
      if (!isLeaderChord(chord, this.leader) && !this.hasNonLeaderPartial(chord)) {
        return this.dispatchResult(e, r, chord)
      }
      this.buffer.push(chord)
      this.notifyBuffer()
      e.preventDefault()
      return { status: "partial" }
    }
    return this.dispatchResult(e, r, chord)
  }

  private chordFromEvent(e: KeyboardEvent): KeyChord {
    // Defer to keystroke normalizer living in the same module boundary
    return normalizeEvent(e)
  }

  /** Returns true when the keyboard event target is an element that accepts
   *  text input (input, textarea, select, contenteditable). */
  private targetIsEditable(e: KeyboardEvent): boolean {
    const target = e.target as HTMLElement | null
    if (!target) return false
    const tag = target.tagName
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true
    if (target.isContentEditable) return true
    return false
  }

  /** Returns true when at least one active scope has a binding reachable
   *  from `chord` whose first key is NOT the leader — i.e. a vim-style
   *  sequence like `gg` that should still work alongside leader bindings. */
  private hasNonLeaderPartial(chord: KeyChord): boolean {
    for (const scope of this.activeScopes) {
      if (this.registry.scope(scope).hasNonLeaderBindingWithPrefix(chord, this.leader)) return true
    }
    return false
  }

  private resolveInScopes(
    scopes: Scope[],
    buffer: KeyChord[],
    e: KeyboardEvent,
  ): ResolveResult {
    for (const scope of scopes) {
      const trie: ScopeTrie = this.registry.scope(scope)
      const eligible = (b: Binding) => {
        if (b.enabled === false) return false

        const ctx: BindingContext = {
          event: e,
          scope,
          focusedPaneId: this.focusedPaneId,
          activeScopes: this.activeScopes,
        }
        if (b.when && !b.when(ctx)) return false
        return true
      }
      const r = trie.resolve(buffer, eligible)
      if (r.status === "match") return r
      if (r.status === "partial") return r
      // 'none' for this scope → try the next lower-priority scope
    }
    return { status: "none", reason: "no-binding" }
  }

  private dispatchResult(e: KeyboardEvent, r: ResolveResult, chord: KeyChord): ResolveResult {
    if (r.status === "match") {
      const b: Binding = (r as any).binding
      e.preventDefault()
      this.resetBuffer()
      const ctx: BindingContext = {
        event: e,
        scope: (r as any).scope,
        focusedPaneId: this.focusedPaneId,
        activeScopes: this.activeScopes,
      }
      try {
        b.action(ctx)
      } catch (err) {
        // Never let a binding throw abort the pipeline
        console.error("[keybinding] action threw for", b.id, err)
      }
      return r
    }
    if (r.status === "partial") {
      // keep the buffer accumulated; wait for next chord or timeout
      return r
    }
    // none — abort the partial sequence
    const aborted = this.buffer
    this.resetBuffer()
    if (aborted.length > 0) this.notifyUnknown(aborted)
    void chord
    return r
  }

  // ── Suspension ─────────────────────────────────────────────────────────────
  // The keystroke-capture UI needs the engine to stand down while recording.
  // Calling preventDefault from the recorder is not enough: this listener sits on
  // `window` in the capture phase, so it runs before any dialog's own listener and
  // would fire the shortcut the user is trying to rebind.

  /** Stop dispatching without detaching. Idempotent. */
  suspend() {
    this.suspended = true
    this.resetBuffer()
  }

  resume() {
    this.suspended = false
  }

  isSuspended(): boolean {
    return this.suspended
  }

  // ── Buffer ──────────────────────────────────────────────────────────────────
  public resetBuffer() {
    const wasEmpty = this.buffer.length === 0
    this.buffer = []
    if (!wasEmpty) this.notifyBuffer()
  }

  // ── Pending-sequence observation ────────────────────────────────────────────
  /** Immutable copy of the current buffer, stable between changes (for
   *  useSyncExternalStore-style consumers). */
  private bufferSnapshot: KeyChord[] = []

  /** Subscribe to pending-sequence buffer changes (for a live chord indicator). */
  subscribeBuffer(fn: (buffer: KeyChord[]) => void): () => void {
    this.bufferListeners.add(fn)
    return () => { this.bufferListeners.delete(fn) }
  }

  private notifyBuffer() {
    this.bufferSnapshot = [...this.buffer]
    if (this.bufferListeners.size === 0) return
    for (const fn of this.bufferListeners) fn(this.bufferSnapshot)
  }

  /** Subscribe to "unknown sequence" events: a leader sequence was typed whose
   *  full buffer resolves to no binding. The chord list includes the leader. */
  subscribeUnknownSequence(fn: (chords: KeyChord[]) => void): () => void {
    this.unknownListeners.add(fn)
    return () => { this.unknownListeners.delete(fn) }
  }

  private unknownListeners: Set<(chords: KeyChord[]) => void> = new Set()

  private notifyUnknown(chords: KeyChord[]) {
    if (this.unknownListeners.size === 0) return
    const copy = [...chords]
    for (const fn of this.unknownListeners) fn(copy)
  }

  // ── Misc ────────────────────────────────────────────────────────────────────
  private focusedPaneId: number | null = null
  setFocusedPane(id: number | null) {
    this.focusedPaneId = id
  }

  /** Snapshot of the current pending buffer (for a UI indicator). Stable
   *  reference between changes. */
  pendingBuffer(): KeyChord[] {
    return this.bufferSnapshot
  }

  /** Possible next chords across the active scopes for a pending buffer, for a
   *  chord affordance hint ("<leader> then: s, w, q"). */
  nextChords(buffer: KeyChord[]): KeyChord[] {
    return this.nextBindings(buffer).map(({ chord }) => chord)
  }

  /**
   * Possible next chords with their binding descriptions. Results are merged by
   * chord and preserve active-scope order, so the UI can render a full hint
   * without coupling the keybinding engine to presentation preferences.
   */
  nextBindings(buffer: KeyChord[]): Array<{ chord: KeyChord; descriptions: string[] }> {
    const out: Array<{ chord: KeyChord; descriptions: string[] }> = []
    const byChord = new Map<string, { chord: KeyChord; descriptions: string[] }>()
    for (const scope of this.activeScopes) {
      for (const binding of this.registry.scope(scope).nextBindings(buffer)) {
        const key = chordKey(binding.chord)
        const existing = byChord.get(key)
        if (existing) {
          if (!existing.descriptions.includes(binding.description)) existing.descriptions.push(binding.description)
        } else {
          const entry = { chord: binding.chord, descriptions: [binding.description] }
          byChord.set(key, entry)
          out.push(entry)
        }
      }
    }
    return out
  }
}
// Re-export the normalizer for engine consumers.
export { normalizeEvent as eventToChord }
