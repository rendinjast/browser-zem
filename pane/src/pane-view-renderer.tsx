import { memo, Suspense, lazy } from 'react'
import { paneRegistry } from './registry'
import type { PaneAppTheme } from './types'

interface PaneViewRendererProps {
  kind: string
  paneId: number
  isFocused: boolean
  sessionId?: string
  params?: Record<string, unknown>
  theme?: PaneAppTheme
}

export const PaneViewRenderer = memo(function PaneViewRenderer({ kind, paneId, isFocused, sessionId, params, theme }: PaneViewRendererProps) {
  const descriptor = paneRegistry.get(kind)
  const componentKey = `${paneId}`

  if (!descriptor) {
    return (
      <div className="flex items-center justify-center w-full h-full text-muted-foreground text-sm">
        Unknown pane: {kind}
      </div>
    )
  }

  // Direct component (built-in panes)
  if (descriptor.component) {
    const Component = descriptor.component
    return <Component key={componentKey} paneId={paneId} isFocused={isFocused} sessionId={sessionId} params={params} theme={theme} />
  }

  // Lazy component (installed extensions)
  if (descriptor.lazyComponent) {
    const LazyComponent = lazy(descriptor.lazyComponent)
    return (
      <Suspense fallback={<PaneLoading />}>
        <LazyComponent key={componentKey} paneId={paneId} isFocused={isFocused} sessionId={sessionId} params={params} theme={theme} />
      </Suspense>
    )
  }

  return (
    <div className="flex items-center justify-center w-full h-full text-muted-foreground text-sm">
      Renderer not available for: {kind}
    </div>
  )
})

function PaneLoading() {
  return (
    <div className="flex items-center justify-center w-full h-full">
      <div className="w-4 h-4 border-2 border-muted-foreground/30 border-t-muted-foreground rounded-full animate-spin" />
    </div>
  )
}
