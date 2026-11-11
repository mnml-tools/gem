# Security Policy

## Supported versions

Gem is pre-1.0. Only the latest release receives security fixes.

## Reporting a vulnerability

Please **do not open a public issue**. Report it privately through
[GitHub Security Advisories](https://github.com/mnml-tools/gem/security/advisories/new)
("Report a vulnerability"), with steps to reproduce and the affected version and OS. You should
get an answer within 7 days.

Never include real hostnames, users, passwords, keys or server contents in a report. Use the
Docker test servers in `dev/servers/` or made-up values to demonstrate the problem.

## Scope

Gem talks only to the servers you connect to. It has no accounts and no telemetry, and it does
not check for updates.

- **Secrets** (passwords, key passphrases) are stored only in your operating system's credential
  store (Keychain, Windows Credential Manager, Secret Service). They never reach Gem's database,
  site exports, the connection log or error messages. If no credential store is available, Gem
  refuses to remember secrets instead of storing them in plain text.
- **SFTP host keys** are verified on every connection (trust on first use). A changed key blocks
  the connection; there is no "accept anyway" button.
- **FTPS certificates** are verified against the system's trusted roots; a self-signed certificate
  must be trusted explicitly by its fingerprint.
- **Plain FTP** sends your password and files unencrypted. Gem marks such sites as "Insecure".
- **Remote files you view** are kept in memory only; Gem does not write them to disk unless you
  download them.
- Gem's database (sites, trusted host keys, settings) is one SQLite file in your user's app-data
  directory, protected by your OS account permissions but **not encrypted**.
- Packages are unsigned, so verify them against the release's `SHA256SUMS.txt` before installing.

The security boundaries (CSP, capabilities, navigation lock, path handling) are described in the
[architecture guide](docs/architecture.md#security-boundaries).
