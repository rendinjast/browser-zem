import type { Binding, KeyChord, KeySeq, LeaderConfig, ResolveResult } from "./types"
import { chordKey, seqEquals } from "./keystroke"
import { isLeaderChord } from "./leader"

// ── Per-scope trie ────────────────────────────────────────────────────────────
// A trie node maps each chord's key => child. Terminal nodes carry the Bindings
// that end there. Bindings are stored (not deduped) so register/unregister by id
// works even when two bindings temporarily share a prefix path.

interface TrieNode {
  children: Map<string, TrieNode>
  bindings: Binding[] // bindings ending exactly at this node
}

function newNode(): TrieNode {
  return { children: new Map(), bindings: [] }
}

/** Depth-first walk: returns true if any binding in the subtree has a first
 *  chord that is NOT the leader. */
function subtreeHasNonLeaderBinding(node: TrieNode, leaderCfg: LeaderConfig): boolean {
  for (const b of node.bindings) {
    if (b.keys.length > 0 && !isLeaderChord(b.keys[0]!, leaderCfg)) return true
  }
  for (const child of node.children.values()) {
    if (subtreeHasNonLeaderBinding(child, leaderCfg)) return true
  }
  return false
}

/**
 * A sequence trie scoped to one Scope. It resolves a typed chord buffer:
 *  - "match" when a binding's full sequence was typed
 *  - "partial" when the buffer is a strict prefix of some binding(s)
 *  - "none" otherwise
 *
 * Among multiple terminal matches, the longest sequence wins (most-specific).
 * Ties among same-length sequences are broken by binding registration order —
 * first registered wins (deterministic). Use distinct ids / scopes to disambiguate.
 */
export class ScopeTrie {
  readonly scope: string
  private root: TrieNode = newNode()
  private byId: Map<string, Binding> = new Map()

  constructor(scope: string) {
    this.scope = scope
  }

  register(binding: Binding): boolean {
    if (this.byId.has(binding.id)) {
      // re-register: remove old path then re-add
      this.unregister(binding.id)
    }
    let node = this.root
    for (const c of binding.keys) {
      const k = chordKey(c)
      const child = node.children.get(k) ?? newNode()
      node.children.set(k, child)
      node = child
    }
    node.bindings.push(binding)
    this.byId.set(binding.id, binding)
    return true
  }

  unregister(id: string): boolean {
    const b = this.byId.get(id)
    if (!b) return false
    this.byId.delete(id)
    // walk path, removing node binding + pruning empty nodes
    const path: { node: TrieNode; key: string }[] = []
    let node = this.root
    for (const c of b.keys) {
      const k = chordKey(c)
      const child = node.children.get(k)
      if (!child) return false
      path.push({ node, key: k })
      node = child
    }
    node.bindings = node.bindings.filter((x) => x.id !== id)
    // prune empty nodes bottom-up
    for (let i = path.length - 1; i >= 0; i--) {
      const { node: parent, key } = path[i]!
      const child = parent.children.get(key)!
      if (child.bindings.length === 0 && child.children.size === 0) {
        parent.children.delete(key)
      } else {
        break
      }
    }
    return true
  }

  clear(): void {
    this.root = newNode()
    this.byId.clear()
  }

  has(id: string): boolean {
    return this.byId.has(id)
  }

  get(id: string): Binding | undefined {
    return this.byId.get(id)
  }

  /** Iterate all registered bindings (no ordering guarantees). */
  *entries(): Iterable<Binding> {
    yield* this.byId.values()
  }

  size(): number {
    return this.byId.size
  }

  /**
   * Resolve a typed buffer. Returns the most-specific match or partial/none.
   * `eligible` filters terminal bindings (used to honor `when` / `enabled`).
   */
  resolve(
    buffer: KeyChord[],
    eligible: (b: Binding) => boolean,
  ): ResolveResult {
    let node = this.root
    for (const c of buffer) {
      const child = node.children.get(chordKey(c))
      if (!child) {
        return { status: "none", reason: "no-binding" }
      }
      node = child
    }
    // node corresponds to exactly `buffer`. Any terminal binding here is a match.
    const matches = node.bindings.filter(eligible)
    if (matches.length > 0) {
      // first registered wins (stable)
      return { status: "match", binding: matches[0]!, scope: this.scope as any }
    }
    // no eligible terminal here — but maybe one with disabled/when-false exists.
    const terminalExists = node.bindings.length > 0
    if (terminalExists) {
      // Some binding *would* match but is disabled / blocked.
      if (node.children.size > 0) return { status: "partial" }
      return { status: "none", reason: "disabled" }
    }
    // node has only children => sequence is a strict prefix
    if (node.children.size > 0) return { status: "partial" }

    // node not found after walking typed buffer (returns early above).
    return { status: "none", reason: "no-binding" }
  }

  /** Walk a one-step buffer to update the engine prefix. Returns true if the
   *  buffer is still a valid prefix across any terminal in this trie. */
  isPrefix(buffer: KeyChord[]): boolean {
    let node = this.root
    for (const c of buffer) {
      const child = node.children.get(chordKey(c))
      if (!child) return false
      node = child
    }
    return true
  }

  /** Distinct chords that can continue the typed `buffer`, for a chord
   *  affordance hint ("<leader> then: s, w, q"). This includes namespace
   *  prefixes whose binding needs additional chords (for example `g` in
   *  `<leader> g 1`). */
  nextChords(buffer: KeyChord[]): KeyChord[] {
    let node = this.root
    for (const c of buffer) {
      const child = node.children.get(chordKey(c))
      if (!child) return []
      node = child
    }
    const out: KeyChord[] = []
    const seen = new Set<string>()
    const visit = (n: TrieNode) => {
      for (const b of n.bindings) {
        if (b.enabled === false) continue
        const next = b.keys[buffer.length]
        if (next && !seen.has(chordKey(next))) {
          seen.add(chordKey(next))
          out.push(next)
        }
      }
      for (const child of n.children.values()) visit(child)
    }
    visit(node)
    return out
  }

  /**
   * Descriptions for bindings completed by exactly one more chord. Longer
   * descendants are namespace prefixes, not immediate actions, so they stay
   * out of the Full hint until the user enters that namespace.
   */
  nextBindings(buffer: KeyChord[]): Array<{ chord: KeyChord; description: string }> {
    let node = this.root
    for (const c of buffer) {
      const child = node.children.get(chordKey(c))
      if (!child) return []
      node = child
    }
    const out: Array<{ chord: KeyChord; description: string }> = []
    const seen = new Set<string>()
    for (const [key, child] of node.children) {
      for (const b of child.bindings) {
        // Disabled bindings should not be advertised by either hint mode.
        // Dynamic `when` predicates are intentionally evaluated by dispatch,
        // since a hint has no KeyboardEvent context to evaluate against.
        if (b.enabled === false || b.keys.length !== buffer.length + 1) continue
        const signature = `${key}\u0000${b.description}`
        if (!seen.has(signature)) {
          seen.add(signature)
          out.push({ chord: b.keys[buffer.length]!, description: b.description })
        }
      }
    }
    return out
  }

  /**
   * Returns true when at least one binding reachable from `chord` does NOT
   * start with the leader.  Used by the engine to allow vim-style sequences
   * (`gg`) while still guarding against stale leader chords (e.g. Space after
   * the leader was changed to Ctrl‑A).
   */
  hasNonLeaderBindingWithPrefix(chord: KeyChord, leaderCfg: LeaderConfig): boolean {
    const node = this.root.children.get(chordKey(chord))
    if (!node) return false
    return subtreeHasNonLeaderBinding(node, leaderCfg)
  }

  /** Direct structural equality for two key sequences among registered bindings. */
  conflictsWith(keys: KeySeq): boolean {
    for (const b of this.byId.values()) {
      if (seqEquals(b.keys, keys)) return true
    }
    return false
  }
}
