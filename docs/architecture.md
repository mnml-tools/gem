# Architecture

Gem is a Tauri 2 desktop app: a React 19 UI in the webview and a Rust core that does all I/O.
The UI never touches the network or the file system; it calls typed commands and receives
session events. This page is the map; decisions and their alternatives are in [`adr/`](adr/), the
UI rules in [`ui-design-system.md`](ui-design-system.md), and the spec in [`../plan.md`](../plan.md).

```
┌──────────────────────── React UI (webview) ─────────────────────────┐
│  sites · connection tabs · file list · viewer/editor · log · queue   │
│  strict CSP · no network · no filesystem                            │
└───────────▲──────────────────────────────────────┬───────────────────┘
            │ Channel<SessionEvent> per tab         │ typed commands (ts-rs bindings)
            │ gem://drag, gem://drop                ▼
┌──────────────────────────── Rust core ───────────────────────────────┐
│ commands/   thin handlers, one permission per command                │
│ services/   sites · secrets · known_hosts · sessions · files ·       │
│             editor · transfers · imports · settings                  │
│ text/       encodings, line endings, lossless round trips            │
│ remote/     RemoteFs trait: sftp/ (russh) · ftp/ (suppaftp+rustls)   │──▶ servers
│ storage/    SQLite: sites, known_hosts, settings                     │
│ platform/   window, navigation lock, OS drag & drop                  │
└──────────────────────────────────────────────────────────────────────┘
```

## UI layers

- `@mnml-tools/ui` (the family kit): tokens, base styles, the theme store and the React
  components Gem uses as is (`AppShell`, `TitleBar`, `StatusBar`, `Tabs`, `Dialog`,
  `ConfirmDialog`, `ErrorDialog`, `ContextMenu`, `Segmented`, `Fields`, `EmptyState`, `Avatar`,
  `Badge`, `StatusDot`, `SearchField`, `ThemeToggle`, `WindowControls`, `useResize`,
  `useShortcut`). Components the kit lacks live in `src/components/ui/` with the kit's look and
  naming, ready to be upstreamed: `VirtualList`, `CodeView` (CodeMirror 6), `SplitPane`,
  `PathBar`, `ProgressBar`, `Toast`.
- `src/views/`: `workspace/` (shell, connection tabs, close guards, window chrome), `sites/`,
  `connection/` (connect flow and prompts, file browser, OS drop), `viewer/`, `panel/` (log and
  transfers), `settings/`.
- `src/state/`: Zustand stores for tabs, file tabs, per-session log buffers and transfers, and UI
  layout. Server data (sites, settings, listings) goes through TanStack Query.
- `src/lib/`: pure helpers with tests (languages, quick connect, formatting, entries, log ring
  buffer, shortcuts); `src/lib/ipc/` the IPC client.
- `src/strings/`: every user-facing string.

### IPC contract

Domain types live in `src-tauri/src/domain/` and are exported with ts-rs to
`src/lib/ipc/bindings/` (`pnpm bindings`; CI fails if they are stale). `src/lib/ipc/api.ts` wraps
every command with typed arguments; UI code never calls `invoke` directly. Errors arrive as
`IpcError { code, message, hostKey }` (`ErrorCode` in `error.rs`). `build.rs` `COMMANDS`,
`lib.rs` `app_commands!` and `capabilities/main.json` list the same commands; the build fails if
the capability misses one.

### Events

Each connection tab passes a `Channel<SessionEvent>` to `connect`. Rust pushes `Log` (batched
every ~16 ms), `State` (connecting / connected / disconnected + reason) and `Transfers` (only the
jobs that changed, at most every ~100 ms). OS drag & drop is the only window-wide event pair:
`gem://drag` and `gem://drop`.

## Rust core

viewer/editor, transfers, sites and their secrets, imports, and OS drag & drop. The protocol
layer itself (`remote/sftp`, `remote/ftp`) is described in its own section.

### Layers

```
commands/    thin #[tauri::command] handlers: resolve state, call a service, return AppResult
services/    sessions · files · editor · transfers · sites · secrets · known_hosts · settings · imports
text/        pure: binary sniffing, encoding detection, windowed decoding, line endings, encoding
remote/      RemoteFs / Connection / HostVerifier traits, path helpers, Logger, memory (tests)
storage/     SQLite (sites, known_hosts, settings), migrations
platform/    window, navigation lock, drag_drop
```

- **Commands are thin.** Database work runs on blocking threads (`commands::support::read/write`);
  native dialogs (`tauri-plugin-dialog` blocking API) run in `spawn_blocking`; remote work is async
  on Tauri's tokio runtime. Commands that touch a session do
  `session.observe(service(&*session.fs()?, …).await)`, so a dropped connection is noticed by any
  operation.
- **Services are testable without Tauri.** Database services are plain functions over `rusqlite`
  (Piw's style); remote services take `&dyn RemoteFs`; sessions take a `Connector` and an
  `EventSink`. Tests use `Database::open_in_memory()`, `MemoryStore` and `remote::memory`
  (`MemoryServer`: POSIX semantics, permissions, mtimes, symlinks, fault injection).
- **Managed state** (lib.rs): `DataState` (the database or why it failed to open),
  `SessionManager` (with `RemoteConnector` and `KeyringStore`), `DropRegistry`.
- **Contract:** domain types in `domain/` are exported with ts-rs to `src/lib/ipc/bindings/`;
  command names/arguments are mirrored in `src/lib/ipc/api.ts`; `build.rs` `COMMANDS`, `lib.rs`
  `app_commands!` and `capabilities/main.json` list the same commands (the build fails otherwise).

### Sessions (ADR 0010)

`SessionManager` maps session ids to `Session`s. A session owns the connection, the primary
`RemoteFs`, the tab's event sink (`Channel<SessionEvent>`), a keepalive task and a transfer queue.

- `connect(target, secret, remember, events)`: resolve the site (saved, re-read from SQLite, or
  ad hoc, validated) → secret (argument → keyring → `SecretRequired` if the method needs one) →
  `remote::connect` with a 20 s timeout and the `KnownHosts` verifier → home and start path (the
  initial path canonicalised, or home with a log line) → store a typed secret if `remember` →
  `SessionInfo` (capabilities, `insecure` for plain FTP, server info).
- Events: `Log` (batched every ~16 ms), `State` (connecting / connected / disconnected + reason),
  `Transfers` (changed jobs, every ~100 ms).
- Keepalive every `keepalive_secs` (0 = off). A failure marks the session disconnected, logs an
  error line and emits `State { disconnected, reason }`; operations then return `Disconnected`.
- `reconnect` keeps the id and the sink; `disconnect` cancels transfers, closes and forgets;
  quitting closes all sessions (3 s budget).
- `test_connection` connects with a collecting logger, reads home, closes and returns a
  `TestResult` with the log. Connection-level failures are results (`ok: false`, `error` with
  `hostKey` when there is one to trust); only invalid input and storage errors reject
  (docs/contract-changes.md).
- Trust errors carry `HostKeyInfo` unchanged. `known_hosts` keeps one identity per host and port
  (lower-cased host); `trust` never overwrites a different stored key (`Conflict`: remove it in the
  site's settings first); `forget` removes it.

### Browsing (`services::files`)

- `list` canonicalises first and returns the canonical path.
- New names (`make_dir`, `make_file`, `rename` target) must be valid single components with no
  `.`/`..` anywhere in the path, so `../x` can never escape the current folder.
- `rename` refuses an existing target; returned entries come from the parent listing, so a renamed
  symlink is still a symlink.
- `delete_paths` is recursive, reads kinds from the parent listing and never follows a symlink into
  a directory (the link is removed). It refuses `/`, the home folder and any folder containing it,
  continues after failures and reports `deleted` + per-path failures.
- `chmod` accepts modes ≤ `0o7777` and requires `capabilities.chmod`.

### Viewer and editor (`services::editor`, `text`, ADR 0013)

- `open_file` stats (folders and special files are refused), reads the first
  `preview_window_kib` window (or the whole file, capped at 512 MiB) and builds `OpenedFile`
  with `text::open`: binary sniffing, encoding, held-back tails, line endings, read-only reason.
  Nothing is written to disk.
- `read_more` re-stats and returns `Conflict` ("The file changed on the server. Reopen it.") if size
  or mtime differ; it decodes with the first window's encoding.
- `save_file` encodes first (UTF-8 ± BOM or windows-1252 only; unmappable characters are a
  `Validation` error with the character and its line), follows symlinks so the link survives,
  checks size/mtime unless `force` (`Conflict`, also when the file was deleted), then
  `write_atomic(path, bytes, original_mode, allow_in_place)` and returns the new stat. `Unsupported`
  from the remote layer means only an in-place overwrite is possible: the UI asks and retries with
  `allow_in_place = true` (`SavedFile.atomic = false`). The UI never sends a save for a file opened
  read-only; the encoding check makes UTF-16 saves impossible anyway.

### Transfers (`services::transfers`, ADR 0011)

Per-session queue, `transfer_concurrency` worker slots, one lazily opened secondary channel per
slot (reused, recreated after a failure). Downloads go to `<name>.gem-part` then are renamed into
place; uploads go to `.<name>.gem-<random>`, then are renamed into place (new files) or moved
over the existing file with `RemoteFs::replace` (keeping the old mode; remove + rename only on a
backend that answers `Unsupported`, with a log line). Folders are expanded when queued, directories created first. Conflict policy from settings
or the user's answer (`apply_to_all` for the rest of the batch); `rename` numbers `name (n).ext`.
Remote names are sanitised for the local OS. A dropped connection fails running jobs and parks
queued ones until a reconnect. Memory is constant: data is streamed in chunks.

Download folder: `settings.download_folder` (only ever set from a native folder dialog;
`update_settings` refuses a new path from the webview) or the OS Downloads folder.

### Sites, secrets and imports (ADR 0012)

- `sites`: validation (name 1–200 chars; host without scheme or spaces; port 1–65535; key auth
  needs a key path; FTP/FTPS forced to password auth; initial path absolute), CRUD, duplicate
  ("<name> copy", secret copied), reorder. `has_secret` follows the keyring, whose work happens
  before the matching database change.
- `secrets`: `KeyringStore` (service `app.gem.Gem`, account = site id). No plain-text fallback.
- `imports`: `~/.ssh/config` (concrete `Host` aliases, first value wins across matching blocks,
  `IdentityFile` with `~` → key auth, else agent), FileZilla `sitemanager.xml` (folders flattened
  as "Folder / Name", `RemoteDir` decoded, `Pass` never read; protocol 0 is imported as explicit
  FTPS), and Gem's JSON (`{"format":"gem-sites","version":1,"sites":[…]}`, no secrets).
  Imports return `SiteInput`s; nothing is saved until the user picks.

### Remote commands and snippets (`services::commands`, `services::snippets`, ADR 0014)

- **Snippets** live in SQLite (`snippets`, schema v2): global (`site_id` NULL) or per site, deleted
  with their site (`ON DELETE CASCADE`). Names 1–100 characters (trimmed), commands 1–4000, no
  NUL. `list_snippets(siteId)` returns the globals, then the site's, each by saved order then
  name; updating `siteId` moves a snippet to the end of the other scope; `reorder_snippets` works
  within one scope.
- **Runs** share their session (ADR 0010): `Session::shell()` is the connection's `RemoteShell`
  (`Disconnected` unless connected, `Unsupported` on FTP), and `SessionInfo.capabilities.exec`
  is `connection.shell().is_some()`, set on connect and reconnect. Each run is an SSH `exec`
  channel without a PTY, registered in the session's `Runs` under a run id; `cancel_command`
  flips its flag (unknown ids are fine) and closing the session stops its runs (waiting up to
  2 s for them to report).
- **`{dir}`** is replaced, every occurrence, by the file list's folder in POSIX single quotes
  (`'` → `'\''`); `{dir}` without a folder is a `validation` error.
- **Output** (`text::terminal::OutputDecoder`, one per stream): lossy UTF-8 that never splits a
  character across chunks, ANSI escape sequences removed (CSI, OSC/DCS strings, two-character
  `ESC` sequences, also when split across chunks), other control characters dropped except `\n`
  and `\t`, `\r\n` → `\n`, and a line redrawn with lone `\r`s (progress bars) shown once in its
  final state. Text is sent in batches every ~30 ms (a partial line is shown at a batch unless it
  is being redrawn), then exactly one `Exit` or `Failed`.
- **Limits:** more than 64 MiB of output stops the run with a note on stderr. A server refusing
  another channel (OpenSSH `MaxSessions`) fails the run with "The server allows no more open
  channels on this connection (MaxSessions)…" and leaves the session connected. Stop sends
  `TERM` and closes the channel after a grace period; when the server reported neither an exit
  status nor a signal, a note on stderr says the program may still be running (no PTY, so no
  hang-up reaches it).
- The connection log gets the command line and how it ended (protocol layer); output never goes
  to any log.

### OS drag & drop (`platform::drag_drop`)

The main window's `WindowEvent::DragDrop` is handled in Rust. Hovering emits `gem://drag`
(`DragHover::Over { x, y }` in CSS pixels, converted with the scale factor, or `Leave`); dropping
stores the paths in `DropRegistry` (16 drops at most, 5 minutes) and emits `gem://drop` with a
`drop_id` and the file names only. `upload_dropped` consumes the id. Local paths never come from
the webview.

### Tests

`cargo test` runs everything without network: `text` (round trips for UTF-8 ± BOM, Latin-1,
CRLF/CR/mixed, no final newline, empty, emoji and `\r\n` split across windows, UTF-16, binary),
`remote::memory`, every service (sessions: secret order, `SecretRequired`, remember only after
success, host-key flow, keepalive → disconnected → reconnect; editor: conflicts, force, encoding
errors, mixed endings, in-place flow, symlinks; transfers: policies, apply-to-all, cancel, retry,
partial-file cleanup, numbering, sanitisation, concurrency limit, channel reuse/recreation,
throttled events, shutdown), imports with fixture strings, the drop registry and the IPC contract
(`commands::tests`: the mock runtime, camelCase arguments as `api.ts` sends them).

`pnpm rust:it` (`GEM_IT=1`, Docker servers) runs the protocol suite (`remote::it`), then
`services::it`: the real `SessionManager` and services end to end for SFTP, FTP, explicit and
implicit FTPS — secrets and trust on first use, every encoding fixture, windows of the 200 MiB
log, byte-exact save round trips, conflicts, the in-place fallback, file operations, folder
transfers both ways, conflict policies, cancels without leftovers and a dropped connection.

## Protocol layer (`remote/`)

One trait, [`RemoteFs`](../src-tauri/src/remote/mod.rs), for every protocol; services never branch
on the protocol and read optional features from `Capabilities`. A `Connection` hands out the
primary file system (browsing, viewer) and secondary ones for transfers, so a transfer never
blocks browsing: a new SFTP channel on the same SSH connection, or a new FTP control connection.

- **SFTP** (`remote/sftp`, `russh` + `russh-sftp`, ring crypto — [ADR 0002](adr/0002-crypto-backend-ring.md)):
  host keys checked in the handshake against the trust store (TOFU; a changed key fails the
  connection with both fingerprints); password, key file (encrypted keys report
  `SecretRequired`) and SSH agent ([ADR 0007](adr/0007-ssh-agent-per-os.md)); pipelined reads and
  writes (16 × 64 KiB in flight).
- **FTP/FTPS** (`remote/ftp`, `suppaftp` + rustls): plain, explicit (`AUTH TLS`) and implicit TLS
  ([ADR 0006](adr/0006-implicit-ftps.md)). Certificates are verified against the Mozilla roots;
  a self-signed certificate is trusted by its SHA-256 fingerprint, like a host key
  ([ADR 0003](adr/0003-ftps-certificate-trust.md)). The control connection is serialised; data
  connections reuse the control TLS session (TLS 1.2 first, see the ADR). `MLSD` when the server
  advertises it, else Gem's own Unix `LIST` parser. Ranged reads use `REST` + `RETR` and resync
  the control connection after an early close; transient `425` replies are retried once before
  any byte moves ([ADR 0004](adr/0004-ftp-ranged-reads-and-replies.md)).
- **Atomic writes** ([ADR 0005](adr/0005-atomic-writes.md)): temp sibling
  `.<name>.gem-<random>`, original mode applied, then `posix-rename@openssh.com` / `RNTO` over the
  target, or a backup dance that restores the original on failure. An in-place overwrite
  happens only with the user's consent (`allow_in_place`). `replace` exposes the same final step
  to the transfer engine. Stale temp siblings of the file being written, older than an hour, are
  removed.

### Connection log

Every backend logs what it does to the session's `Logger`: `status`, `command`, `response`,
`error`. FTP `PASS`/`ACCT` arguments are masked by `Logger::command`; no other line ever carries a
password, passphrase, key material or file contents (the integration suite captures every line
of full sessions and checks). The protocol crates' own `log` output is pinned to `Warn` in
`lib.rs` because suppaftp traces raw control lines, including `PASS`.

## Security boundaries

- **Webview:** CSP `default-src 'self'`, `connect-src ipc: http://ipc.localhost`, no inline
  scripts, `freezePrototype`, `withGlobalTauri: false`, no `plugin:*` JavaScript permissions.
  The one capability (`capabilities/main.json`) grants each app command plus window controls.
- **Navigation lock:** the window only shows the app's own pages; new windows are denied
  (`platform/navigation.rs`).
- **Local paths** come only from native dialogs or OS drops handled in Rust; the webview sees
  file names and drop ids, never paths. Download names from the server are sanitised for the
  local OS.
- **Secrets** live only in the OS keyring (`app.gem.Gem`, account = site id); never in SQLite,
  exports, logs or errors. No plain-text fallback.
- **Remote content** is always rendered as text by CodeMirror; nothing from a server is
  interpreted as HTML or executed. Viewed files stay in memory.
- **Trust:** SFTP host keys and FTPS certificates are pinned per host and port; a change blocks
  the connection with no override in the dialog.

## Data

SQLite at `<app data dir>/gem.sqlite3` (`GEM_DATA_DIR` overrides it): `sites`, `known_hosts`,
`settings`; WAL, `PRAGMA user_version` migrations with a backup before upgrades, and a refusal to
open a database from a newer Gem. The theme and accent are kept by the kit's theme store in the
webview (`gem.theme`, `gem.accent`).

## Validation

```sh
pnpm verify          # typecheck, lint, Vitest, cargo fmt/clippy/test, build
pnpm servers:up      # Docker SFTP / FTP / FTPS test servers (dev/servers/README.md)
pnpm rust:it         # protocol and service integration suites against them (GEM_IT=1)
pnpm servers:down
```

CI runs the same: frontend and Rust checks on Linux, Windows and macOS, the MSRV check, and the
protocol integration job with the Docker servers.

### Browser preview

`pnpm dev` and open <http://localhost:1426>: outside Tauri, `src/dev/` mocks every command in
memory (never bundled by `pnpm build`). See [ui-design-system.md §7](ui-design-system.md#7-browser-preview).
