import { useRef, useEffect, useCallback } from 'react'
import { usePaneContext } from './pane-provider'

/**
 * Retry a focus/blur operation across a few animation frames so targets that
 * mount asynchronously (terminal canvas after PTY init, CodeMirror on first
 * render) still receive/relinquish DOM focus. Re-running on a settled target
 * is a no-op, so the extra calls are harmless.
 */
function retriedCall(run: () => void, enabled: () => boolean) {
  let frames = 0
  const step = () => {
    if (!enabled()) return
    run()
    frames += 1
    if (frames < 4) requestAnimationFrame(step)
  }
  step()
}

export function usePane(
  paneId: number,
  isFocused: boolean,
  onFocus?: () => void,
  onPaneAction?: (action: string, path?: string, params?: Record<string, unknown>) => void,
  onBlur?: () => void,
) {
  const rootRef = useRef<HTMLDivElement>(null)
  const { focusPaneById } = usePaneContext()
  const onFocusRef = useRef(onFocus)
  const onPaneActionRef = useRef(onPaneAction)
  const onBlurRef = useRef(onBlur)
  const isFocusedRef = useRef(isFocused)
  // Bumped on every focus-state change so a pending retry from a previous
  // state is cancelled the instant the pane gains/loses focus again.
  const focusCycleRef = useRef(0)

  onFocusRef.current = onFocus
  onPaneActionRef.current = onPaneAction
  onBlurRef.current = onBlur
  isFocusedRef.current = isFocused

  const handlePointerDown = useCallback(() => {
    focusPaneById(paneId)
  }, [paneId, focusPaneById])

  useEffect(() => {
    const el = rootRef.current
    if (!el) return
    const onPointerDown = () => handlePointerDown()
    el.addEventListener('pointerdown', onPointerDown, { capture: true })
    return () => el.removeEventListener('pointerdown', onPointerDown)
  }, [handlePointerDown])

  // Keep the browser's DOM focus in sync with this pane's logical focus. The
  // two are decoupled: keystrokes go to the element with DOM focus, so they
  // must land in the focused pane's editor/terminal — retrying until that
  // async-mounted target is ready. When the pane loses logical focus, release
  // DOM focus so a stale editor/terminal can't keep swallowing keystrokes.
  useEffect(() => {
    const token = ++focusCycleRef.current
    if (isFocused) {
      retriedCall(
        () => onFocusRef.current?.(),
        () => isFocusedRef.current && focusCycleRef.current === token,
      )
    } else {
      retriedCall(
        () => onBlurRef.current?.(),
        () => !isFocusedRef.current && focusCycleRef.current === token,
      )
    }
  }, [isFocused])

  useEffect(() => {
    const handler = (e: Event) => {
      const detail = (e as CustomEvent).detail
      if (detail.paneId !== paneId) return
      onPaneActionRef.current?.(detail.action, detail.path, detail.params)
    }
    window.addEventListener('pane-action', handler)
    return () => window.removeEventListener('pane-action', handler)
  }, [paneId])

  return { rootRef }
}
