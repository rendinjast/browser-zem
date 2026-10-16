/** Add a scheme when the user typed a bare hostname/domain. */
export function normalizeUrl(raw: string): string {
  let s = raw.trim()
  if (!s) return ''
  // Drop stray whitespace (e.g. "google .com") which yields invalid URLs.
  s = s.replace(/\s+/g, '')
  if (/^(https?|file|about|data):/i.test(s)) return s
  // Localhost and bare IPs are overwhelmingly http; everything else https.
  if (/^(localhost|127\.|10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/i.test(s)) {
    return `http://${s}`
  }
  return `https://${s}`
}

/** Extract the hostname (without port) from a URL. */
export function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '')
  } catch {
    return url
  }
}
