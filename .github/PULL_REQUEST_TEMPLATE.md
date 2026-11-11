## What and why

<!-- What does this change, and which user flow does it improve? Link issues with "Closes #…". -->

## How it was tested

<!-- Commands you ran and their results. Name the OS/desktop for anything platform-specific. -->

- [ ] `pnpm verify` (typecheck, lint, tests, Rust fmt/clippy/tests, build)
- [ ] `pnpm servers:up && pnpm rust:it` if the protocol layer changed
- [ ] Manually checked on: <!-- e.g. Linux KDE Wayland, Windows 11 — or "not platform-specific" -->

## Checklist

- [ ] No remote or local data can be lost (atomic writes, conflict checks, confirmations, migrations tested)
- [ ] No secret can reach a log line, SQLite, a JSON export or an error message
- [ ] UI is keyboard-accessible and follows `docs/ui-design-system.md`
- [ ] User-facing strings are in `src/strings/`
- [ ] `pnpm bindings` run if Rust DTOs changed
- [ ] Docs updated (`README.md`, `docs/architecture.md`, CHANGELOG under "Unreleased")
