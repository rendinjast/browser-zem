import type { KeyChord, KeySeq } from "./types"

// ─── Normalization: KeyboardEvent -> KeyChord ─────────────────────────────────
// We lower-case the key so `S` / `s` collapse, and we strip the shift modifier
// when the key is already a shifted symbol (e.g. Shift+/ => '?') to avoid
// requiring callers to write both forms.

const SHIFTED_MAP: Record<string, string> = {
  "`": "~", "1": "!", "2": "@", "3": "#", "4": "$", "5": "%",
  "6": "^", "7": "&", "8": "*", "9": "(", "0": ")",
  "-": "_", "=": "+", "[": "{", "]": "}", "\\": "|",
  ";": ":", "'": "\"", ",": "<", ".": ">", "/": "?",
}
const SHIFTED_INVERSE: Record<string, string> = Object.fromEntries(
  Object.entries(SHIFTED_MAP).map(([k, v]) => [v, k]),
)

/**
 * The single canonical form of a chord. Every chord entering the system — from a
 * KeyboardEvent, a spec string, or a persisted user keymap — passes through here,
 * so the trie is keyed one way and lookups can't miss.
 *
 * The rule: a shifted symbol carries its own shift. `Shift+[` *is* `{`, so it
 * canonicalizes to `{ key: '{' }`, never `{ key: '[', shift: true }`. Letters,
 * space and arrows keep `shift`, because there `shift` is a real modifier rather
 * than a different glyph — that's what makes `h` and `H` distinct bindings.
 */
export function canonicalizeChord(c: KeyChord): KeyChord {
  let key = c.key.toLowerCase()
  let shift = !!c.shift

  if (shift && key in SHIFTED_MAP) {
    // base char + shift => the shifted glyph, which now implies the shift
    key = SHIFTED_MAP[key]!
    shift = false
  } else if (key in SHIFTED_INVERSE) {
    // already a shifted glyph ('?', '{', ':') — shift is redundant
    shift = false
  }

  const out: KeyChord = { key }
  if (c.ctrl) out.ctrl = true
  if (c.meta) out.meta = true
  if (c.alt) out.alt = true
  if (shift) out.shift = true
  return out
}

export function canonicalizeSeq(s: KeySeq): KeySeq {
  return s.map(canonicalizeChord)
}

export function eventToChord(e: KeyboardEvent): KeyChord {
  const chord: KeyChord = { key: e.key.toLowerCase() }
  if (e.ctrlKey) chord.ctrl = true
  if (e.metaKey) chord.meta = true
  if (e.altKey) chord.alt = true
  if (e.shiftKey) chord.shift = true
  return canonicalizeChord(chord)
}

// ─── Spec-string parser ───────────────────────────────────────────────────────
// Grammar (tokens joined by '+', sequences by ' '):
//   chord  := mod ('+' mod)* ('+' key)
//   seq    := chord (' ' chord)*
//   mod    := cmd | ctrl | alt | opt | shift | super
//   key    := literal (until '+' or space) or <special>
//   special := <leader> | <space> | <tab> | <enter> | <esc(ape)> | <up> | ...
//
// Examples:
//   "cmd+o"
//   "ctrl+shift+n"
//   "<leader> s"          (two chord sequence)
//   "g t"                (two chord sequence)
//   "shift+h"

export interface ParseOptions {
  /** Expands the <leader> token to these chord(s) when encountered. */
  leader?: KeyChord[]
  throwOnError?: boolean
}

const MOD_ALIASES: Record<string, "ctrl" | "meta" | "alt" | "shift"> = {
  cmd: "meta", command: "meta", super: "meta", win: "meta",
  ctrl: "ctrl", control: "ctrl",
  alt: "alt", opt: "alt", option: "alt",
  shift: "shift",
}

const SPECIAL_KEYS: Record<string, string> = {
  space: " ",
  plus: "+", // '+' is the modifier separator, so it can only be named

  tab: "tab",
  enter: "enter", return: "enter", ret: "enter",
  esc: "escape", escape: "escape",
  backspace: "backspace",
  up: "arrowup", down: "arrowdown", left: "arrowleft", right: "arrowright",
  del: "delete", delete: "delete",
  home: "home", end: "end",
  pgup: "pageup", pageup: "pageup", pgdn: "pagedown", pagedown: "pagedown",
}

function tokenizeSeq(spec: string): string[][] {
  // Split into chord groups by whitespace; each group by '+'
  return spec
    .trim()
    .split(/\s+/)
    .filter(Boolean)
    .map((group) => group.split("+").map((t) => t.trim()).filter(Boolean))
}

export function parseChord(group: string[], opts: ParseOptions = {}): KeyChord[] {
  // Returns possibly multiple chords when this token is <leader> (expands).
  // <leader> may appear as its own single-token group; if it's mixed with mods
  // (e.g. "cmd+<leader>") we attach the mods to the first leader chord.
  const chord: KeyChord = { key: "" }

  for (const tok of group) {
    const lower = tok.toLowerCase()
    if (lower in MOD_ALIASES) {
      const m = MOD_ALIASES[lower]!
      ;(chord as any)[m] = true
      continue
    }
    if (lower.startsWith("<") && lower.endsWith(">")) {
      const name = lower.slice(1, -1)
      if (name === "leader") {
        // Carry over only the modifiers typed alongside <leader> ("cmd+<leader>").
        // `chord.key` is still the empty placeholder here and must not overwrite
        // the leader's own key.
        const { key: _placeholder, ...mods } = chord
        return (opts.leader ?? [{ key: " " }]).map((c) =>
          canonicalizeChord({ ...c, ...mods }),
        )
      }
      const mapped = SPECIAL_KEYS[name]
      if (mapped === undefined) {
        if (opts.throwOnError) throw new Error(`Unknown special key <${name}>`)
        // unknown — treat as literal 'name'
        return [canonicalizeChord({ ...chord, key: name })]
      }
      return [canonicalizeChord({ ...chord, key: mapped })]
    }
    // Named keys are accepted bare as well as bracketed: "ctrl+space" is what
    // people (and exported keymaps) actually write, and without this it would
    // parse to the literal key "space" and never match a real keypress.
    const named = SPECIAL_KEYS[lower]
    if (named !== undefined) return [canonicalizeChord({ ...chord, key: named })]

    // Literal key. Only the capital-letter case is decided here (`H` => shift+h);
    // shifted symbols are settled by canonicalizeChord so that spec strings and
    // KeyboardEvents land on the same canonical chord.
    let key = tok
    if (key.length === 1 && key === key.toUpperCase() && key !== key.toLowerCase()) {
      key = key.toLowerCase()
      chord.shift = true
    } else {
      key = key.toLowerCase()
    }
    return [canonicalizeChord({ ...chord, key })]
  }

  // Every branch above returns as soon as a non-modifier token is seen, so
  // reaching here means the group was modifiers only ("cmd+", "shift"). There is
  // no key to bind; yield nothing rather than a keyless chord that would occupy
  // a trie slot no keystroke can ever reach.
  if (opts.throwOnError) throw new Error(`Chord has no key: ${group.join("+")}`)
  return []
}

export function parseSpec(spec: string, opts: ParseOptions = {}): KeySeq {
  const groups = tokenizeSeq(spec)
  const seq: KeySeq = []
  for (const g of groups) {
    const chords = parseChord(g, opts)
    seq.push(...chords)
  }
  if (seq.length === 0 && opts.throwOnError) {
    throw new Error(`Empty key spec: ${JSON.stringify(spec)}`)
  }
  return seq
}

// ─── Equality / hashing ───────────────────────────────────────────────────────

export function chordEquals(a: KeyChord, b: KeyChord): boolean {
  return (
    a.key === b.key &&
    !!a.ctrl === !!b.ctrl &&
    !!a.meta === !!b.meta &&
    !!a.alt === !!b.alt &&
    !!a.shift === !!b.shift
  )
}

export function seqEquals(a: KeySeq, b: KeySeq): boolean {
  if (a.length !== b.length) return false
  return a.every((c, i) => chordEquals(c, b[i]!))
}

/** Stable string key for use in maps/sets. */
export function chordKey(c: KeyChord): string {
  return [
    c.ctrl ? "C" : "",
    c.meta ? "M" : "",
    c.alt ? "A" : "",
    c.shift ? "S" : "",
    c.key,
  ].filter(Boolean).join("+")
}

export function seqKey(s: KeySeq): string {
  return s.map(chordKey).join(" › ")
}

// ─── Spec serialization (the inverse of parseSpec) ────────────────────────────
// `chordKey`/`seqKey` are internal map keys and deliberately not reparseable.
// These produce the persisted, hand-editable form instead, so that
// parseSpec(seqToSpec(seq)) === canonicalizeSeq(seq) for every sequence.

/** Keys whose canonical form differs from the token parseSpec expects. */
const SPEC_KEY_NAMES: Record<string, string> = {
  " ": "space",
  "+": "plus", // '+' is the modifier separator, so it can only be named
  arrowup: "up", arrowdown: "down", arrowleft: "left", arrowright: "right",
}

export function chordToSpec(c: KeyChord): string {
  const parts: string[] = []
  if (c.ctrl) parts.push("ctrl")
  if (c.meta) parts.push("cmd")
  if (c.alt) parts.push("alt")
  if (c.shift) parts.push("shift")
  parts.push(SPEC_KEY_NAMES[c.key] ?? c.key)
  return parts.join("+")
}

/** Serialize a sequence to a spec string ("cmd+k cmd+s"). */
export function seqToSpec(s: KeySeq): string {
  return s.map(chordToSpec).join(" ")
}

// ─── Display formatting (platform-aware) ──────────────────────────────────────

export interface FormatOptions {
  /** Use '⌘' on mac, 'Ctrl' elsewhere. Defaults to navigator-based detection. */
  mac?: boolean
}

function isMac(): boolean {
  if (typeof navigator !== "undefined") return /mac|iphone|ipad/i.test(navigator.platform || navigator.userAgent || "")
  return false
}

export function formatChord(c: KeyChord, opts: FormatOptions = {}): string[] {
  const mac = opts.mac ?? isMac()
  const parts: string[] = []
  if (c.ctrl) parts.push(mac ? "⌃" : "Ctrl")
  if (c.meta) parts.push(mac ? "⌘" : "Super")
  if (c.alt) parts.push(mac ? "⌥" : "Alt")
  if (c.shift) parts.push("⇧")
  parts.push(prettyKey(c.key))
  return parts
}

export function formatSeq(s: KeySeq, opts: FormatOptions = {}): string[] {
  return s.flatMap((c) => formatChord(c, opts))
}

export function prettyKey(key: string): string {
  switch (key) {
    case " ": return "Space"
    case "tab": return "Tab"
    case "enter": return "Return"
    case "escape": return "Esc"
    case "backspace": return "⌫"
    case "arrowup": return "↑"
    case "arrowdown": return "↓"
    case "arrowleft": return "←"
    case "arrowright": return "→"
    case "delete": return "Del"
    case "home": return "Home"
    case "end": return "End"
    case "pageup": return "PgUp"
    case "pagedown": return "PgDn"
    default: {
      // capitalize single letters, leave symbols alone
      if (key.length === 1 && /[a-z]/.test(key)) return key.toUpperCase()
      return key
    }
  }
}

// ─── Is the incoming KeyboardEvent a plain modifier press? ────────────────────
// We ignore lone modifier events so they don't start/abort sequence buffers
// noisily.

const MODIFIER_KEYS = new Set([
  "control", "alt", "shift", "meta", "command", "fn", "capslock",
])

export function isStandaloneModifier(e: KeyboardEvent): boolean {
  return MODIFIER_KEYS.has(e.key.toLowerCase())
}
