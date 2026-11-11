//! Saved sites (connections). Secrets never live here: see `services::secrets`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum Protocol {
    Sftp,
    Ftp,
    Ftps,
}

impl Protocol {
    pub fn default_port(self, ftps_mode: FtpsMode) -> u16 {
        match (self, ftps_mode) {
            (Self::Sftp, _) => 22,
            (Self::Ftps, FtpsMode::Implicit) => 990,
            (Self::Ftp | Self::Ftps, _) => 21,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sftp => "sftp",
            Self::Ftp => "ftp",
            Self::Ftps => "ftps",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "sftp" => Some(Self::Sftp),
            "ftp" => Some(Self::Ftp),
            "ftps" => Some(Self::Ftps),
            _ => None,
        }
    }

    /// Plain FTP sends credentials and data in clear text.
    pub fn is_insecure(self) -> bool {
        matches!(self, Self::Ftp)
    }
}

/// How an SFTP site authenticates. FTP and FTPS always use `password` (an empty user means
/// anonymous).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum AuthMethod {
    Password,
    /// A private key file, optionally protected by a passphrase (the secret).
    Key,
    /// The running SSH agent (`SSH_AUTH_SOCK` / Pageant).
    Agent,
}

impl AuthMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Key => "key",
            Self::Agent => "agent",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "password" => Some(Self::Password),
            "key" => Some(Self::Key),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum FtpsMode {
    /// `AUTH TLS` on the control connection (port 21).
    Explicit,
    /// TLS from the first byte (port 990).
    Implicit,
}

impl FtpsMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Implicit => "implicit",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "explicit" => Some(Self::Explicit),
            "implicit" => Some(Self::Implicit),
            _ => None,
        }
    }
}

/// Avatar colours for sites (mapped to kit tokens in the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum SiteColor {
    Gray,
    Red,
    Orange,
    Yellow,
    Green,
    Teal,
    Blue,
    Purple,
    Pink,
}

impl SiteColor {
    pub const ALL: [Self; 9] = [
        Self::Gray,
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Teal,
        Self::Blue,
        Self::Purple,
        Self::Pink,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gray => "gray",
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Yellow => "yellow",
            Self::Green => "green",
            Self::Teal => "teal",
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Pink => "pink",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == value)
    }
}

/// What the site dialog sends to create or update a site, and what quick connect builds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct SiteInput {
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthMethod,
    /// Private key path for `auth = key` (chosen with a native dialog).
    pub key_path: Option<String>,
    pub ftps_mode: FtpsMode,
    /// FTP passive mode (default on; active mode rarely works through NAT).
    pub passive: bool,
    /// Remote directory to open after connecting; the server's home when empty.
    pub initial_path: Option<String>,
    pub color: Option<SiteColor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct Site {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthMethod,
    pub key_path: Option<String>,
    pub ftps_mode: FtpsMode,
    pub passive: bool,
    pub initial_path: Option<String>,
    pub color: Option<SiteColor>,
    pub sort_order: i64,
    /// Whether a password or passphrase is stored in the OS keyring for this site.
    pub has_secret: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl Site {
    pub fn input(&self) -> SiteInput {
        SiteInput {
            name: self.name.clone(),
            protocol: self.protocol,
            host: self.host.clone(),
            port: self.port,
            user: self.user.clone(),
            auth: self.auth,
            key_path: self.key_path.clone(),
            ftps_mode: self.ftps_mode,
            passive: self.passive,
            initial_path: self.initial_path.clone(),
            color: self.color,
        }
    }
}

impl SiteInput {
    /// Whether connecting needs a secret (password or key passphrase) from the keyring or user.
    /// A key may or may not be encrypted; the remote layer reports `SecretRequired` then.
    pub fn needs_password(&self) -> bool {
        match self.protocol {
            Protocol::Sftp => self.auth == AuthMethod::Password,
            Protocol::Ftp | Protocol::Ftps => !self.user.is_empty() && self.user != "anonymous",
        }
    }
}

/// What to do with a site's stored secret when it is saved.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum SecretUpdate {
    Keep,
    Clear,
    Set { value: String },
}
