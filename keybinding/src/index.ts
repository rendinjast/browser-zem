// Public API for @zem/keybinding
export type {
  Scope, KeyChord, KeySeq, Binding, BindingContext, BindingSource,
  ConflictRecord, RegistrySnapshot, ResolveResult, LeaderConfig,
} from "./types"
export { SCOPE_PRIORITY, scopeRank } from "./types"
export {
  eventToChord, parseSpec, parseChord, chordEquals, seqEquals,
  chordKey, seqKey, formatChord, formatSeq, prettyKey, isStandaloneModifier,
  canonicalizeChord, canonicalizeSeq, chordToSpec, seqToSpec,
} from "./keystroke"
export type { ParseOptions, FormatOptions } from "./keystroke"
export { ScopeTrie } from "./trie"
export { KeybindingRegistry, orderedScopes } from "./registry"
export { KeybindingEngine } from "./engine"
export type { EngineOptions } from "./engine"
export {
  DEFAULT_LEADER,
  resolveLeaderConfig, isLeaderChord,
} from "./leader"

// React conveniences (optional peer dep)
export {
  useActiveScopes, useBinding, useRegistrySnapshot, useFocusedPaneSync,
  usePendingSequence,
} from "./react"
