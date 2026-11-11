# Test servers

Local SFTP, FTP and FTPS servers for Gem's protocol integration tests and for UI demos. They all
serve **the same tree** at the same absolute paths (`/home/gem/…`), from one shared Docker volume.

> **Everything here is test-only.** The user, password, passphrase, SSH keys and TLS
> certificates exist only for these containers. Never reuse them anywhere else. All ports are
> bound to `127.0.0.1`.

## Use

```sh
pnpm servers:up     # build, start and wait until every server is healthy
pnpm rust:it        # GEM_IT=1: remote/it.rs (protocols), then services/it.rs (end to end)
pnpm servers:down   # stop and delete the containers and the volume
```

`GEM_IT_LOG=1 pnpm rust:it` also prints every connection log line, to debug a failing suite.
The first start generates the large fixtures (about 10 s); the volume is reset by
`servers:down`.

## Servers

| Service   | Software                  | Port (127.0.0.1) | Protocols                               | Listing |
| --------- | ------------------------- | ---------------- | --------------------------------------- | ------- |
| `sftp`    | OpenSSH 10 (Alpine)       | `2222`           | SFTP                                    | —       |
| `ftp`     | vsftpd 3.0.5              | `2121`           | FTP, FTPS explicit (`AUTH TLS`)         | `LIST`  |
| `ftp`     | vsftpd 3.0.5 (2nd daemon) | `2990`           | FTPS implicit (TLS from the first byte) | `LIST`  |
| `proftpd` | ProFTPD 1.3.9 + mod_tls   | `2321`           | FTP, FTPS explicit                      | `MLSD`  |
| `seed`    | Alpine (one-shot)         | —                | fills the shared volume, then exits     | —       |

Passive data ports (published one to one, the servers announce `127.0.0.1`): vsftpd
`30000–30039` (port 2121) and `30040–30079` (port 2990), ProFTPD `30080–30159`. Active mode
cannot work through Docker's NAT and is not exercised.

Both FTP servers keep their defaults for TLS session reuse: data connections must resume the
control connection's TLS session (`require_ssl_reuse=YES` in vsftpd, ProFTPD's default), as on
most production servers. Host keys and certificates are generated when the images are built, so
a rebuild changes them (the tests trust them on first use).

## Credentials (test-only)

| What                              | Value / file                                                       |
| --------------------------------- | ------------------------------------------------------------------ |
| User                              | `gem` (uid 1000, home `/home/gem`)                                 |
| Password (SFTP and FTP)           | `gem-test-password`                                                |
| SSH key, no passphrase            | `keys/id_ed25519`                                                  |
| SSH key, encrypted                | `keys/id_ed25519_encrypted`, passphrase `gem-test-passphrase`      |
| SSH RSA key (rsa-sha2 signatures) | `keys/id_rsa` (also loaded into a private agent by the agent test) |
| Authorised keys                   | `keys/authorized_keys` (the three public keys)                     |

Git does not keep `0600` on the private keys; Gem does not need it, and the agent test copies
the key with `0600` before `ssh-add`.

## The tree (`/home/gem`)

| Path                                                         | Source    | What it tests                                             |
| ------------------------------------------------------------ | --------- | --------------------------------------------------------- |
| `samples/`                                                   | committed | One file per v1 language (see below)                      |
| `encodings/latin1-crlf.txt`                                  | committed | Windows-1252 text with accents and `€`, CRLF line endings |
| `encodings/latin1.txt`                                       | committed | ISO-8859-1 text, LF                                       |
| `encodings/crlf.txt`                                         | committed | ASCII, CRLF                                               |
| `encodings/mixed-eol.txt`                                    | committed | LF, CRLF and CR in one file, no final newline             |
| `encodings/utf16le-bom.txt`                                  | committed | UTF-16LE with BOM                                         |
| `encodings/utf8.txt`, `utf8-bom.txt`, `no-final-newline.txt` | committed | UTF-8 without/with BOM, missing final newline             |
| `logs/app.log`                                               | generated | 200 MiB log, every byte verifiable from its offset        |
| `logs/small.log`                                             | generated | A tiny log                                                |
| `binary/bytes.bin`                                           | generated | 64 KiB: byte values 0–255 repeated                        |
| `binary/pixel.png`                                           | generated | A 1×1 PNG                                                 |
| `many/`                                                      | generated | 10 000 empty files (`file-00001.txt` …)                   |
| `readonly/`                                                  | generated | Root-owned folder: nothing can be created (see below)     |
| `link-to-dir`, `link-to-file`, `link-broken`                 | generated | Symlinks to a folder, a file, nothing                     |
| `names/`                                                     | generated | Spaces, accents, CJK and emoji in names                   |
| `it/`                                                        | generated | Scratch space, one subfolder per test suite               |

Line _n_ (from 0) of `logs/app.log` starts at byte _n_·128; its format is in `seeder/seed.sh` and
rebuilt by the tests (`log_bytes` in `it.rs`). In `readonly/`, `locked.txt` (owned by root)
cannot be written at all, while `editable.txt` (owned by gem) can only be overwritten in place:
the non-atomic fallback. ProFTPD leaves the dangling `link-broken` out of MLSD listings.

`samples/` holds `index.php`, `main.go`, `docker-compose.yml`, `.env`, `composer.json`,
`Dockerfile`, `deploy.sh` (mode 755), `nginx.conf`, `config.yaml`, `app.py`, `lib.rs`, `style.css`,
`page.html` (with a script that must never run), `data.xml`, `query.sql`, `README.md`,
`config.toml`, `.htaccess`, `Makefile` (tab-indented) and `app.ts` (plan §2.3).

Committed fixtures live in `seed/` and are kept byte for byte: `seed/.gitattributes` turns off
line-ending conversion, `.prettierignore` skips the folder and `.gitignore` here re-includes the
synthetic `.env`. Large and Unicode-named fixtures are generated at start instead of committed.

## Files

- `docker-compose.yml` — the services, healthchecks and the shared `home` volume.
- `seeder/` — the one-shot image and `seed.sh` that builds the tree.
- `sftp/` — OpenSSH image and `sshd_config` (password + public key, internal SFTP).
- `ftp/` — vsftpd image, the two configurations (`vsftpd.conf` for 21, `vsftpd-implicit.conf`
  for 990) and the entrypoint that supervises both daemons.
- `proftpd/` — ProFTPD image and `proftpd.conf` (mod_tls, MLSD).
- `keys/` — the test-only SSH keys.
