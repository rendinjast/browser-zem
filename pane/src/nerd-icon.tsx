const nerdIcons: Record<string, string> = {
  terminal: '\uf120',
  folder: '\uf07b',
  chart: '\uf080',
  square: '\uf152',
  plus: '\uf067',
}

export function NerdIcon({ icon, className }: { icon: keyof typeof nerdIcons; className?: string }) {
  const code = nerdIcons[icon]
  if (!code) return null
  return (
    <span
      className={`size-full flex items-center justify-center leading-none ${className ?? ''}`}
      style={{ fontFamily: "'SymbolsNerdFontMono', monospace" }}
    >
      {code}
    </span>
  )
}