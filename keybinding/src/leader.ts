import type { KeyChord, LeaderConfig } from "./types"
import { chordKey } from "./keystroke"

export const DEFAULT_LEADER: KeyChord = { key: "a", ctrl: true } // Ctrl+A, like tmux/emacs style

export function resolveLeaderConfig(cfg?: Partial<LeaderConfig>): LeaderConfig {
  return {
    keys: cfg?.keys && cfg.keys.length > 0 ? cfg.keys : [DEFAULT_LEADER],
  }
}

export function isLeaderChord(chord: KeyChord, cfg: LeaderConfig): boolean {
  return cfg.keys.some((l) => chordKey(l) === chordKey(chord))
}
