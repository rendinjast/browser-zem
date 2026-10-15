import { createContext, useContext } from 'react'

interface PaneContextValue {
  focusPaneById: (id: number) => void
}

const PaneContext = createContext<PaneContextValue>({
  focusPaneById: () => {},
})

export function PaneProvider({
  focusPaneById,
  children,
}: {
  focusPaneById: (id: number) => void
  children: React.ReactNode
}) {
  return (
    <PaneContext.Provider value={{ focusPaneById }}>
      {children}
    </PaneContext.Provider>
  )
}

export function usePaneContext(): PaneContextValue {
  return useContext(PaneContext)
}
