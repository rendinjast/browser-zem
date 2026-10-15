// React conveniences for @zem/keybinding. The package marks react as an optional
// peer dependency; types are resolved via the consuming app's node_modules.

import { useEffect, useRef, useSyncExternalStore } from "react"
import type { Binding, KeyChord, Scope } from "./types"
import { KeybindingRegistry } from "./registry"
import { KeybindingEngine } from "./engine"

type SubscribeFn = (onStoreChange: () => void) => (() => void)

/** Activate `scopes` on the engine for as long as the component is mounted and
 *  `active` is true. Scopes are reference-counted by the engine's set semantics,
 *  so sibling components activating the same scope are safe. */
export function useActiveScopes(engine: KeybindingEngine, scopes: Scope[], active = true) {
  const key = scopes.join("|")
  useEffect(() => {
    if (!active) return
    const list = key ? (key.split("|") as Scope[]) : []
    for (const s of list) engine.pushScope(s)
    return () => {
      for (const s of list) engine.popScope(s)
    }
  }, [engine, key, active])
}

export function useBinding(engine: KeybindingEngine, binding: Binding, deps: unknown[] = []) {
  const ref = useRef(binding)
  ref.current = binding
  useEffect(() => {
    engine.registry.register(ref.current)
    return () => { engine.registry.unregister(ref.current.id) }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [binding.id, ...deps])
}

export function useRegistrySnapshot(registry: KeybindingRegistry) {
  return useSyncExternalStore(
    ((onStoreChange: () => void) => registry.subscribe(onStoreChange)) as SubscribeFn,
    () => registry.snapshot(),
    () => registry.snapshot(),
  )
}

/** Live pending sequence buffer — non-empty while a chord sequence is mid-flight
 *  (e.g. after typing the leader). Drives the on-screen pending-sequence
 *  indicator. */
export function usePendingSequence(engine: KeybindingEngine): KeyChord[] {
  return useSyncExternalStore(
    (onStoreChange) => engine.subscribeBuffer(onStoreChange),
    () => engine.pendingBuffer(),
    () => engine.pendingBuffer(),
  )
}

export function useFocusedPaneSync(engine: KeybindingEngine, paneId: number | null) {
  useEffect(() => {
    engine.setFocusedPane(paneId)
  }, [engine, paneId])
}
