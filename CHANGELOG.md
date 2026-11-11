# Changelog

All notable changes are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Gem uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **App icon** (an emerald-cut gem in the family's sticker style, on yellow) for the packages and
  the window; the title bar shows it as a one-colour glyph.
- **Connections in tabs:** SFTP (password, key file with passphrase, SSH agent), FTP and FTPS
  (explicit and implicit). Several tabs can be open at once, also to the same site; each keeps
  its own folder, open files, log and transfers.
- **Sites page:** saved sites with colours, quick connect (`sftp://user@host:port/path`), a site
  dialog with "Test connection" that shows the server conversation, and imports from
  `~/.ssh/config`, FileZilla's `sitemanager.xml` and Gem's JSON export. Passwords and key
  passphrases are kept only in the system keyring.
- **Trust on first use:** SFTP host keys and self-signed FTPS certificates are shown with their
  SHA-256 fingerprint before the first connection; a changed key blocks the connection.
  Plain FTP sites are marked "Insecure".
- **Remote file list:** breadcrumbs that turn into an editable path, sorting, filtering, hidden
  files, symlinks, multi-select, keyboard navigation, rename, new folder and file, recursive
  delete with a report, and permissions. Folders with tens of thousands of entries stay fluid.
- **View without downloading:** Space previews a file, Enter pins it. Files are read into memory
  only and shown with syntax highlighting, indentation guides, folding and search for PHP, Go,
  YAML, JSON, JavaScript/TypeScript, HTML, CSS/SCSS, XML, SQL, Python, Rust, Shell, Markdown,
  INI/`.env`/properties, TOML, nginx, Apache, Dockerfile, Makefile and logs, in the family's code
  colours. Large files load in windows ("Load more", "Load all").
- **Edit and save back safely:** the server copy is checked for changes before saving
  (Overwrite / Reload / Cancel), the file is written to a temp sibling and renamed over the
  original with its permissions, and the encoding (UTF-8 with or without BOM, Windows-1252),
  line endings and final newline are preserved.
- **Transfers:** uploads by drag and drop from the system or "Upload…", downloads to the
  download folder or "Download to…", whole folders, a queue with progress, cancel, retry and
  Overwrite / Skip / Rename for existing files. Partial files are never left behind.
- **Remote commands over SSH** (SFTP sites): run snippets or any command with Ctrl+K or the
  Snippets menu; the output opens in a tab next to your files, streamed, with stderr marked, the
  exit status, search, Re-run and Stop. Snippets are global or per site, can use `{dir}` for the
  current folder, and can ask before running. Recent commands are kept in memory only.
- **Connection log at the bottom,** with commands, replies and errors, passwords always masked,
  copy and "Save log…".
- Light and dark mode and five accents, shared with the other mnml-tools apps.
