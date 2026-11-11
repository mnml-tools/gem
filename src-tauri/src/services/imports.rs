//! Site imports and exports (plan §4.5). Nothing here saves anything: imports return
//! [`SiteInput`]s for the import dialog, which creates the sites the user picks.
//!
//! - `~/.ssh/config`: one SFTP site per concrete `Host` alias, with options resolved the way
//!   `ssh` does (every matching block, first value wins).
//! - FileZilla's `sitemanager.xml`: folders flattened into "Folder / Name". Passwords are never
//!   read, even when the file contains them.
//! - Gem's own JSON export (`{"format":"gem-sites","version":1,"sites":[…]}`), no secrets.

use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::sites;
use crate::domain::site::{AuthMethod, FtpsMode, Protocol, Site, SiteInput};
use crate::error::{AppError, AppResult};

pub const EXPORT_FORMAT: &str = "gem-sites";
pub const EXPORT_VERSION: u32 = 1;

const MAX_NAME: usize = 200;

fn site(name: &str, protocol: Protocol, host: String, port: u16, user: String) -> SiteInput {
    SiteInput {
        name: name.chars().take(MAX_NAME).collect(),
        protocol,
        host,
        port,
        user,
        auth: AuthMethod::Password,
        key_path: None,
        ftps_mode: FtpsMode::Explicit,
        passive: true,
        initial_path: None,
        color: None,
    }
}

// ----- ~/.ssh/config ---------------------------------------------------------------------

struct Block {
    /// `None` for `Match` blocks, which Gem does not evaluate (they never match).
    patterns: Option<Vec<String>>,
    options: Vec<(String, String)>,
}

impl Block {
    fn matches(&self, alias: &str) -> bool {
        let Some(patterns) = &self.patterns else {
            return false;
        };
        let alias = alias.to_ascii_lowercase();
        let mut positive = false;
        for pattern in patterns {
            let pattern = pattern.to_ascii_lowercase();
            if let Some(negated) = pattern.strip_prefix('!') {
                if glob(negated.as_bytes(), alias.as_bytes()) {
                    return false;
                }
            } else if glob(pattern.as_bytes(), alias.as_bytes()) {
                positive = true;
            }
        }
        positive
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// `*` and `?` wildcards, as in `ssh_config` patterns.
fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            glob(&pattern[1..], text) || (!text.is_empty() && glob(pattern, &text[1..]))
        }
        (Some(b'?'), Some(_)) => glob(&pattern[1..], &text[1..]),
        (Some(p), Some(t)) if p == t => glob(&pattern[1..], &text[1..]),
        _ => false,
    }
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
        .to_owned()
}

fn expand_home(path: &str, home: &str) -> String {
    let path = path.replace("%d", home);
    if path == "~" {
        home.to_owned()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{rest}", home.trim_end_matches(['/', '\\']))
    } else {
        path
    }
}

/// Parses an `ssh_config`. `home` expands `~`/`%d`; `default_user` is used when a host has
/// no `User` (ssh uses the local user name). `Include` is not followed.
pub fn parse_ssh_config(text: &str, home: &str, default_user: &str) -> Vec<SiteInput> {
    // Options before the first Host apply to every host.
    let mut blocks = vec![Block {
        patterns: Some(vec!["*".into()]),
        options: Vec::new(),
    }];
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let split = line
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(line.len());
        let key = line[..split].to_ascii_lowercase();
        let value = line[split..]
            .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
            .trim();
        match key.as_str() {
            "host" => blocks.push(Block {
                patterns: Some(value.split_whitespace().map(unquote).collect()),
                options: Vec::new(),
            }),
            "match" => blocks.push(Block {
                patterns: None,
                options: Vec::new(),
            }),
            _ => {
                if let Some(block) = blocks.last_mut() {
                    block.options.push((key, unquote(value)));
                }
            }
        }
    }

    let mut aliases: Vec<String> = Vec::new();
    for block in &blocks[1..] {
        for pattern in block.patterns.iter().flatten() {
            let concrete = !pattern.contains(['*', '?', '!']);
            if concrete && !aliases.contains(pattern) {
                aliases.push(pattern.clone());
            }
        }
    }

    let mut sites = Vec::new();
    for alias in aliases {
        // First obtained value wins, in file order, over every block that matches.
        let lookup = |key: &str| {
            blocks
                .iter()
                .filter(|b| b.matches(&alias))
                .find_map(|b| b.get(key))
                .map(str::to_owned)
        };
        let host = lookup("hostname")
            .map(|h| h.replace("%h", &alias).replace("%%", "%"))
            .unwrap_or_else(|| alias.clone());
        let user = lookup("user").unwrap_or_else(|| default_user.to_owned());
        let port = lookup("port").and_then(|p| p.parse().ok()).unwrap_or(22);
        let identity = lookup("identityfile")
            .filter(|p| !p.eq_ignore_ascii_case("none"))
            .map(|p| expand_home(&p, home));
        let mut input = site(&alias, Protocol::Sftp, host, port, user);
        match identity {
            Some(path) => {
                input.auth = AuthMethod::Key;
                input.key_path = Some(path);
            }
            None => input.auth = AuthMethod::Agent,
        }
        match sites::normalise(input) {
            Ok(input) => sites.push(input),
            Err(error) => log::info!("skipping an ssh_config host: {error}"),
        }
    }
    sites
}

// ----- FileZilla -------------------------------------------------------------------------

#[derive(Default)]
struct FzServer {
    fields: std::collections::HashMap<String, String>,
    /// Old files keep the site name as text directly inside `<Server>`.
    text: String,
}

/// The FileZilla fields Gem reads. `Pass` (and everything else) is never stored.
const FZ_FIELDS: &[&str] = &[
    "Host",
    "Port",
    "Protocol",
    "User",
    "Logontype",
    "Name",
    "RemoteDir",
    "Keyfile",
    "PasvMode",
];

/// Parses FileZilla's `sitemanager.xml`.
pub fn parse_filezilla(xml: &str) -> AppResult<Vec<SiteInput>> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut folders: Vec<String> = Vec::new();
    let mut server: Option<FzServer> = None;
    let mut sites = Vec::new();
    let invalid = |e: quick_xml::Error| {
        AppError::validation(format!("this is not a valid FileZilla site file: {e}"))
    };
    loop {
        let text: Option<String> = match reader.read_event().map_err(invalid)? {
            Event::Start(start) => {
                let name = start.name().into_inner().to_owned();
                match name.as_str() {
                    "Folder" => folders.push(String::new()),
                    "Server" => server = Some(FzServer::default()),
                    _ => {}
                }
                stack.push(name);
                None
            }
            Event::End(_) => {
                match stack.pop().as_deref() {
                    Some("Folder") => {
                        folders.pop();
                    }
                    Some("Server") => {
                        if let Some(done) = server.take()
                            && let Some(site) = filezilla_site(&done, &folders)
                        {
                            sites.push(site);
                        }
                    }
                    _ => {}
                }
                None
            }
            Event::Text(text) => Some(text.xml10_content().into_owned()),
            Event::CData(data) => Some(data.to_string()),
            Event::GeneralRef(reference) => match reference.resolve_char_ref() {
                Ok(Some(c)) => Some(c.to_string()),
                _ => match reference.xml10_content().as_ref() {
                    "amp" => Some("&".into()),
                    "lt" => Some("<".into()),
                    "gt" => Some(">".into()),
                    "quot" => Some("\"".into()),
                    "apos" => Some("'".into()),
                    _ => None,
                },
            },
            Event::Eof => break,
            _ => None,
        };
        let Some(text) = text else {
            continue;
        };
        let depth = stack.len();
        match (
            stack.last().map(String::as_str),
            depth.checked_sub(2).map(|i| stack[i].as_str()),
        ) {
            (Some("Folder"), _) => {
                if let Some(name) = folders.last_mut() {
                    name.push_str(&text);
                }
            }
            (Some("Server"), _) => {
                if let Some(server) = server.as_mut() {
                    server.text.push_str(&text);
                }
            }
            (Some(field), Some("Server")) if FZ_FIELDS.contains(&field) => {
                if let Some(server) = server.as_mut() {
                    server
                        .fields
                        .entry(field.to_owned())
                        .or_default()
                        .push_str(&text);
                }
            }
            _ => {}
        }
    }
    Ok(sites)
}

fn filezilla_site(server: &FzServer, folders: &[String]) -> Option<SiteInput> {
    let field = |name: &str| server.fields.get(name).map(|v| v.trim()).unwrap_or("");
    let (protocol, ftps_mode) = match field("Protocol") {
        // 0 is FileZilla's default "FTP: use explicit TLS if available"; Gem has no
        // opportunistic mode, so it imports as explicit FTPS (switch to FTP if the server
        // has no TLS). 6 is "plain FTP (insecure)".
        "" | "0" | "4" => (Protocol::Ftps, FtpsMode::Explicit),
        "1" => (Protocol::Sftp, FtpsMode::Explicit),
        "3" => (Protocol::Ftps, FtpsMode::Implicit),
        "6" => (Protocol::Ftp, FtpsMode::Explicit),
        _ => return None, // HTTP, S3, WebDAV, …
    };
    let host = field("Host").to_owned();
    let port = field("Port")
        .parse()
        .unwrap_or_else(|_| protocol.default_port(ftps_mode));
    let logon = field("Logontype");
    let user = if logon == "0" {
        String::new() // anonymous
    } else {
        field("User").to_owned()
    };
    let own_name = Some(field("Name"))
        .filter(|n| !n.is_empty())
        .unwrap_or(server.text.trim());
    let own_name = if own_name.is_empty() {
        host.as_str()
    } else {
        own_name
    };
    let mut parts: Vec<&str> = folders
        .iter()
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .collect();
    parts.push(own_name);
    let mut input = site(&parts.join(" / "), protocol, host, port, user);
    input.ftps_mode = ftps_mode;
    if protocol == Protocol::Sftp && logon == "5" && !field("Keyfile").is_empty() {
        input.auth = AuthMethod::Key;
        input.key_path = Some(field("Keyfile").to_owned());
    }
    input.passive = field("PasvMode") != "MODE_ACTIVE";
    input.initial_path = parse_remote_dir(field("RemoteDir"));
    match sites::normalise(input) {
        Ok(input) => Some(input),
        Err(error) => {
            log::info!("skipping a FileZilla site: {error}");
            None
        }
    }
}

/// Decodes FileZilla's serialised server path: `<type> <len> <prefix> <len> <segment>…`,
/// e.g. `1 0 3 var 3 www` is `/var/www` on a Unix server. Other server types are ignored.
fn parse_remote_dir(encoded: &str) -> Option<String> {
    let (kind, mut rest) = encoded.split_once(' ')?;
    if kind != "0" && kind != "1" {
        return None;
    }
    let mut tokens = Vec::new();
    while !rest.is_empty() {
        let digits = rest.find(' ').unwrap_or(rest.len());
        let len: usize = rest[..digits].parse().ok()?;
        rest = rest.get(digits + 1..).unwrap_or("");
        let end = rest.char_indices().nth(len).map_or(rest.len(), |(i, _)| i);
        if rest[..end].chars().count() != len {
            return None;
        }
        tokens.push(&rest[..end]);
        rest = rest[end..].strip_prefix(' ').unwrap_or(&rest[end..]);
    }
    // The first token is the path prefix (used by VMS); the rest are the segments.
    let segments = tokens.get(1..)?;
    if segments.is_empty() {
        return Some("/".into());
    }
    Some(format!("/{}", segments.join("/")))
}

// ----- Gem JSON ----------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Export {
    format: String,
    version: u32,
    sites: Vec<SiteInput>,
}

/// Every site, without ids or secrets.
pub fn export_json(sites: &[Site]) -> AppResult<String> {
    serde_json::to_string_pretty(&Export {
        format: EXPORT_FORMAT.into(),
        version: EXPORT_VERSION,
        sites: sites.iter().map(Site::input).collect(),
    })
    .map_err(|e| AppError::Internal(format!("export: {e}")))
}

/// Reads a Gem export; every site is validated.
pub fn import_json(text: &str) -> AppResult<Vec<SiteInput>> {
    let not_ours = || AppError::validation("this is not a Gem sites file");
    let value: Value = serde_json::from_str(text).map_err(|_| not_ours())?;
    if value.get("format").and_then(Value::as_str) != Some(EXPORT_FORMAT) {
        return Err(not_ours());
    }
    let version = value.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version > u64::from(EXPORT_VERSION) {
        return Err(AppError::validation(
            "this sites file was written by a newer version of Gem",
        ));
    }
    if version == 0 {
        return Err(not_ours());
    }
    let export: Export = serde_json::from_value(value)
        .map_err(|e| AppError::validation(format!("the sites file is damaged: {e}")))?;
    export
        .sites
        .into_iter()
        .enumerate()
        .map(|(index, input)| {
            let name = input.name.clone();
            sites::normalise(input)
                .map_err(|e| AppError::validation(format!("site {} ({name}): {e}", index + 1)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SSH_CONFIG: &str = r#"
# Global defaults
ServerAliveInterval 30

Host prod prod-alias
    HostName 203.0.113.10
    User deploy
    Port 2222
    IdentityFile ~/.ssh/id_prod

Host staging
  HostName=%h.example.test
  User = "web user"

Host *.internal !secret.internal
    User ops

Host build.internal
    IdentityFile "/keys/build key"

Match host prod
    User nobody

Host *
    User fallback
    IdentityFile none
"#;

    #[test]
    fn ssh_config_hosts_with_first_value_wins() {
        let sites = parse_ssh_config(SSH_CONFIG, "/home/me", "me");
        let names: Vec<&str> = sites.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["prod", "prod-alias", "staging", "build.internal"]);
        let prod = &sites[0];
        assert_eq!(prod.host, "203.0.113.10");
        assert_eq!((prod.user.as_str(), prod.port), ("deploy", 2222));
        assert_eq!(prod.auth, AuthMethod::Key);
        assert_eq!(prod.key_path.as_deref(), Some("/home/me/.ssh/id_prod"));
        assert_eq!(prod.protocol, Protocol::Sftp);
        let staging = &sites[2];
        assert_eq!(staging.host, "staging.example.test");
        assert_eq!(staging.user, "web user");
        assert_eq!(staging.port, 22);
        assert_eq!(staging.auth, AuthMethod::Agent, "IdentityFile none");
        let build = &sites[3];
        assert_eq!(build.user, "ops", "from the *.internal block");
        assert_eq!(build.key_path.as_deref(), Some("/keys/build key"));
    }

    #[test]
    fn ssh_config_defaults_to_the_local_user() {
        let sites = parse_ssh_config("Host box\n  HostName box.test\n", "/home/me", "me");
        assert_eq!(sites[0].user, "me");
        assert!(parse_ssh_config("", "/home/me", "me").is_empty());
    }

    #[test]
    fn glob_patterns() {
        assert!(glob(b"*.internal", b"build.internal"));
        assert!(glob(b"web?", b"web1"));
        assert!(!glob(b"web?", b"web12"));
        assert!(glob(b"*", b""));
    }

    const FILEZILLA: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?>
<FileZilla3 version="3.66.0" platform="linux">
  <Servers>
    <Server>
      <Host>ftp.example.test</Host>
      <Port>21</Port>
      <Protocol>0</Protocol>
      <Type>0</Type>
      <User>alice</User>
      <Pass encoding="base64">c2VjcmV0LXBhc3N3b3Jk</Pass>
      <Logontype>1</Logontype>
      <PasvMode>MODE_ACTIVE</PasvMode>
      <Name>Tom &amp; Jerry&#x27;s site</Name>
      <RemoteDir>1 0 3 var 3 www 8 my files</RemoteDir>
      <LocalDir>/home/alice/www</LocalDir>
    </Server>
    <Folder expanded="1">Clients
      <Folder>Acme
        <Server>
          <Host>sftp.acme.test</Host>
          <Port>2222</Port>
          <Protocol>1</Protocol>
          <User>deploy</User>
          <Logontype>5</Logontype>
          <Keyfile>/home/alice/.ssh/acme</Keyfile>
          <Name>Production</Name>
        </Server>
      </Folder>
      <Server>
        <Host>secure.example.test</Host>
        <Protocol>3</Protocol>
        <Logontype>0</Logontype>
        <User>ignored</User>
        Legacy name
      </Server>
      <Server>
        <Host>s3.example.test</Host>
        <Protocol>7</Protocol>
        <Name>Bucket</Name>
      </Server>
    </Folder>
    <Server><Host>plain.example.test</Host><Protocol>6</Protocol><Name>Plain</Name></Server>
  </Servers>
</FileZilla3>"#;

    #[test]
    fn filezilla_sites_without_passwords() {
        let sites = parse_filezilla(FILEZILLA).unwrap();
        let names: Vec<&str> = sites.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Tom & Jerry's site",
                "Clients / Acme / Production",
                "Clients / Legacy name",
                "Plain"
            ]
        );
        let first = &sites[0];
        assert_eq!(
            (first.protocol, first.ftps_mode),
            (Protocol::Ftps, FtpsMode::Explicit)
        );
        assert_eq!(first.user, "alice");
        assert!(!first.passive);
        assert_eq!(first.initial_path.as_deref(), Some("/var/www/my files"));
        let acme = &sites[1];
        assert_eq!(acme.protocol, Protocol::Sftp);
        assert_eq!(acme.port, 2222);
        assert_eq!(acme.auth, AuthMethod::Key);
        assert_eq!(acme.key_path.as_deref(), Some("/home/alice/.ssh/acme"));
        let legacy = &sites[2];
        assert_eq!(
            (legacy.protocol, legacy.ftps_mode),
            (Protocol::Ftps, FtpsMode::Implicit)
        );
        assert_eq!(legacy.port, 990);
        assert_eq!(legacy.user, "", "anonymous");
        assert_eq!(sites[3].protocol, Protocol::Ftp);
        // The password never makes it into anything that is returned.
        let dump = format!("{sites:?}");
        assert!(!dump.contains("c2VjcmV0") && !dump.contains("secret-password"));
    }

    #[test]
    fn filezilla_rejects_garbage() {
        assert!(parse_filezilla("<FileZilla3><Servers><Server></Folder>").is_err());
        assert!(parse_filezilla("<other/>").unwrap().is_empty());
    }

    #[test]
    fn remote_dir_encoding() {
        assert_eq!(
            parse_remote_dir("1 0 4 home 4 user").as_deref(),
            Some("/home/user")
        );
        assert_eq!(parse_remote_dir("1 0").as_deref(), Some("/"));
        assert_eq!(parse_remote_dir("1 0 5 a b c").as_deref(), Some("/a b c"));
        assert_eq!(parse_remote_dir("8 0 3 abc"), None, "not a Unix path");
        assert_eq!(parse_remote_dir(""), None);
        assert_eq!(parse_remote_dir("1 0 9 short"), None);
    }

    #[test]
    fn json_round_trip_and_versioning() {
        let db = crate::storage::Database::open_in_memory().unwrap();
        let store = crate::services::secrets::MemoryStore::default();
        let input = crate::services::test_support::sftp_input();
        sites::create(&db, &store, input.clone(), Some("pw".into())).unwrap();
        let all = db.read(sites::list).unwrap();
        let json = export_json(&all).unwrap();
        assert!(json.contains("\"format\": \"gem-sites\""));
        assert!(
            !json.contains("pw\"") && !json.contains("hasSecret") && !json.contains(&all[0].id)
        );
        assert_eq!(import_json(&json).unwrap(), [input]);

        assert!(import_json("{}").is_err());
        assert!(import_json("not json").is_err());
        let newer = json.replace("\"version\": 1", "\"version\": 2");
        assert!(
            import_json(&newer)
                .unwrap_err()
                .to_string()
                .contains("newer")
        );
        let broken = json.replace("\"host\": \"example.test\"", "\"host\": \"\"");
        assert!(
            import_json(&broken)
                .unwrap_err()
                .to_string()
                .contains("site 1 (prod)")
        );
    }
}
