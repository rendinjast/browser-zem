# browser-zem

Browser pane for [zem](https://github.com/rendinjast/zem) — a webview pane with
history, bookmarks and an embedded devtools drawer. Ships as two installable
pieces:

| Piece | What it does | Installed by |
| --- | --- | --- |
| `lib/` + `Cargo.toml` — **server addon** (`com.rendinjast.browser`) | Embedded HTTP proxy, navigation state, bookmarks, actions | `zem addon install` → GitHub release |
| `src/` + `package.json` — **web extension** (`zem.browser-pane`) | The pane UI bundle served to the web client | `POST /api/extension/install` → GitHub release |

Both are published from **one GitHub Release** of this repo.

## Layout

```
browser-zem/
├── lib/                    Rust addon sources (main.rs, pane.rs, proxy.rs, bridge.js)
├── src/                    Vite/React pane sources (index.tsx, …)
├── Cargo.toml              Rust package manifest (binary: addon)
├── package.json            JS package manifest (pane bundle)
├── pane-manifest.toml      extension install manifest
├── vite.config.ts
├── tsconfig.json
└── .github/workflows/      release pipeline
```

The zem monorepo packages this repo depends on (`zem-addon-sdk`,
`zem-addon-protocol`, `@zem-sh/pane`, `@zem-sh/keybinding`) are consumed as
regular registry dependencies.

## Build

```sh
# server addon (release binary at target/release/addon)
cargo build --release
cargo test

# web pane bundle (dist/index.js)
pnpm install
pnpm build
```

Generate the addon manifest from code (the release pipeline does this):

```sh
./target/release/addon --generate-manifest
```

## Release

Push a version tag; CI builds everything and publishes a GitHub Release:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The repo must be **public**: zem's installers download release assets
unauthenticated, so a private repo would 404 on install.

### Release asset contract

zem's installers hard-code these asset names — do not rename them:

| Asset | Consumed by |
| --- | --- |
| `browser-macos` / `browser-linux` | addon installer (`{name}-{os}`, os = Rust `std::env::consts::OS`) |
| `zem-addon-manifest.toml` | addon installer (exact name required) |
| `pane-manifest.toml` | extension installer (exact name required) |
| `index.js` | extension installer (the `pane.entry` from pane-manifest) |

**Upload order matters**: the server infers the addon name from the *first*
release asset (text before the last `-`), so the platform binaries must be
uploaded first (`browser-macos` → `browser`). The release workflow enforces
this order.

## Install (server addon)

```sh
# latest release
zem addon install rendinjast/browser-zem

# a specific version (tag must be v{version})
zem addon install rendinjast/browser-zem@0.1.0
```

Then **restart the zem server** — the addon manager only loads addons from
`~/.zem/addon/` at startup, so a freshly installed addon appears after a
restart (`AddonManager::load_all`).

Verify / manage:

```sh
zem addon ls                                  # contains com.rendinjast.browser
zem addon update com.rendinjast.browser       # pull the latest release again
zem addon uninstall com.rendinjast.browser --force
```

Note: the addon id must **not** start with `com.zem.` — zem treats those ids
as built-in core addons and refuses to uninstall them.

## Install (web extension)

Admin only (header = an admin user's UUID from `~/.zem/connections.json`):

```sh
curl -X POST http://127.0.0.1:50051/api/extension/install \
  -H "Content-Type: application/json" \
  -H "X-User-UUID: <admin-uuid>" \
  -d '{"source":"rendinjast/browser-zem"}'
```

Verify / manage:

```sh
curl -H "X-User-UUID: <admin-uuid>" http://127.0.0.1:50051/api/extension
curl -X DELETE -H "X-User-UUID: <admin-uuid>" \
  http://127.0.0.1:50051/api/extension/zem.browser-pane
```

The files land in `~/.zem/extensions/zem.browser-pane/` and are served at
`/api/extension/zem.browser-pane/index.js`.

### Known limitation

zem's web client only loads pane bundles listed in its hard-coded
`BUILT_IN_PANE_DEFS` (`apps/web/src/lib/builtin-panes.ts` in the zem
monorepo). Until a `browser` entry (id `zem.browser-pane`, kind `browser`) is
added there, installing the extension puts everything in place and the
catalog/serve endpoints work, but the pane will not appear in the UI.
