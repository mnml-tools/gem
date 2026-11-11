//! Directory listing parsers: MLSD/MLST facts (RFC 3659) and `LIST` lines.
//!
//! Unix `ls -l` lines are parsed here rather than with suppaftp's parser because Gem needs the
//! full mode (setuid, setgid and sticky bits included, so a save never drops them) and the
//! owner and group names; suppaftp's parser keeps only the nine rwx bits and numeric ids. Lines
//! that are not Unix-style (DOS/IIS) go through suppaftp's parser.

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeZone, Utc};

use crate::domain::entry::{Entry, EntryKind};
use crate::remote::path;

/// One parsed listing line, before it is placed in a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub link_target: Option<String>,
    /// MLSD `cdir`/`pdir` entries and `.`/`..`: skipped by listings.
    pub is_dot: bool,
}

impl Listed {
    pub fn into_entry(self, dir: &str) -> Entry {
        let path = path::join(dir, &self.name);
        self.into_entry_at(path)
    }

    pub fn into_entry_at(self, path: String) -> Entry {
        Entry {
            name: path::file_name(&path).to_owned(),
            path,
            kind: self.kind,
            size: self.size,
            mtime: self.mtime,
            mode: self.mode,
            owner: self.owner,
            group: self.group,
            link_target: self.link_target,
            target_kind: None,
        }
    }
}

/// Parses one MLSD line or MLST fact line: `fact=value;fact=value; name`.
pub fn parse_mlsx(line: &str) -> Option<Listed> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (facts, name) = line.split_once(' ')?;
    if name.is_empty() {
        return None;
    }
    let mut listed = Listed {
        name: name.to_owned(),
        kind: EntryKind::File,
        size: None,
        mtime: None,
        mode: None,
        owner: None,
        group: None,
        link_target: None,
        is_dot: name == "." || name == "..",
    };
    let (mut owner_id, mut group_id) = (None, None);
    for fact in facts.split(';').filter(|fact| !fact.is_empty()) {
        let Some((key, value)) = fact.split_once('=') else {
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "type" => {
                let lower = value.to_ascii_lowercase();
                listed.kind = match lower.as_str() {
                    "file" => EntryKind::File,
                    "dir" => EntryKind::Dir,
                    "cdir" | "pdir" => {
                        listed.is_dot = true;
                        EntryKind::Dir
                    }
                    "os.unix=symlink" => EntryKind::Symlink,
                    other if other.starts_with("os.unix=slink") => {
                        // `OS.unix=slink:/target` (case of the target preserved).
                        listed.link_target = value
                            .split_once(':')
                            .map(|(_, target)| target.to_owned())
                            .filter(|target| !target.is_empty());
                        EntryKind::Symlink
                    }
                    _ => EntryKind::Other,
                };
            }
            "size" | "sizd" => listed.size = value.parse().ok(),
            "modify" => listed.mtime = parse_mlsx_time(value),
            "unix.mode" => {
                listed.mode = u32::from_str_radix(value, 8).ok().map(|mode| mode & 0o7777);
            }
            "unix.ownername" => listed.owner = Some(value.to_owned()),
            "unix.groupname" => listed.group = Some(value.to_owned()),
            "unix.owner" | "unix.uid" => owner_id = Some(value.to_owned()),
            "unix.group" | "unix.gid" => group_id = Some(value.to_owned()),
            _ => {}
        }
    }
    listed.owner = listed.owner.or(owner_id);
    listed.group = listed.group.or(group_id);
    if listed.kind != EntryKind::File {
        // Directory and link sizes are meaningless here (block or link-text size).
        listed.size = None;
    }
    Some(listed)
}

/// `YYYYMMDDHHMMSS[.sss]` in UTC (RFC 3659 `time-val`), as Unix seconds.
pub fn parse_mlsx_time(value: &str) -> Option<i64> {
    let digits = value.split('.').next()?;
    NaiveDateTime::parse_from_str(digits, "%Y%m%d%H%M%S")
        .ok()
        .map(|time| time.and_utc().timestamp())
}

/// Parses one `LIST` line (Unix `ls -l` style, or DOS through suppaftp). `None` for lines that
/// are not entries (`total 42`, blank lines).
pub fn parse_list_line(line: &str, now: DateTime<Utc>) -> Option<Listed> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.trim().is_empty() {
        return None;
    }
    parse_unix_line(line, now).or_else(|| parse_with_suppaftp(line))
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn month(token: &str) -> Option<u32> {
    let lower = token.to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|name| *name == lower)
        .and_then(|index| u32::try_from(index + 1).ok())
}

/// Byte spans of the whitespace-separated tokens of `line`.
fn tokens(line: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, ch) in line.char_indices() {
        match (ch.is_whitespace(), start) {
            (true, Some(begin)) => {
                spans.push((begin, index));
                start = None;
            }
            (false, None) => start = Some(index),
            _ => {}
        }
    }
    if let Some(begin) = start {
        spans.push((begin, line.len()));
    }
    spans
}

/// The 12 permission bits from the 9 `rwxrwxrwx` characters (with `s`/`S`/`t`/`T`).
fn mode_bits(perms: &str) -> Option<u32> {
    let chars: Vec<char> = perms.chars().collect();
    if chars.len() != 9 {
        return None;
    }
    let mut mode = 0u32;
    for (triad, special) in [(0usize, 0o4000u32), (1, 0o2000), (2, 0o1000)] {
        let shift = 6 - 3 * triad as u32;
        let [r, w, x] = [chars[3 * triad], chars[3 * triad + 1], chars[3 * triad + 2]];
        match r {
            'r' => mode |= 4 << shift,
            '-' => {}
            _ => return None,
        }
        match w {
            'w' => mode |= 2 << shift,
            '-' => {}
            _ => return None,
        }
        match x {
            'x' => mode |= 1 << shift,
            's' | 't' => mode |= (1 << shift) | special,
            'S' | 'T' => mode |= special,
            '-' => {}
            _ => return None,
        }
    }
    Some(mode)
}

fn parse_unix_line(line: &str, now: DateTime<Utc>) -> Option<Listed> {
    let spans = tokens(line);
    let token = |index: usize| spans.get(index).map(|&(start, end)| &line[start..end]);
    let mode_token = token(0)?;
    // `-rw-r--r--`, optionally followed by an ACL/xattr marker (`+`, `@`, `.`).
    let mut chars = mode_token.chars();
    let kind = match chars.next()? {
        '-' => EntryKind::File,
        'd' => EntryKind::Dir,
        'l' => EntryKind::Symlink,
        'c' | 'b' | 'p' | 's' | 'D' => EntryKind::Other,
        _ => return None,
    };
    let perms: String = chars.by_ref().take(9).collect();
    let mode = mode_bits(&perms)?;
    let rest: String = chars.collect();
    if !rest.is_empty() && !matches!(rest.as_str(), "+" | "@" | ".") {
        return None;
    }
    // Find "<size> <Mon> <day> <HH:MM|year> <name>"; the fields before the size are the link
    // count (when numeric), the owner and the group (either may be missing).
    let date_at = (2..spans.len().saturating_sub(3)).find(|&index| {
        token(index).and_then(month).is_some()
            && token(index + 1)
                .is_some_and(|day| day.parse::<u32>().is_ok_and(|day| (1..=31).contains(&day)))
            && token(index + 2).is_some_and(is_time_or_year)
    })?;
    let size_token = token(date_at - 1)?;
    let mut fields: Vec<&str> = (1..date_at - 1).filter_map(token).collect();
    let size = if size_token.chars().all(|c| c.is_ascii_digit()) {
        size_token.parse().ok()
    } else {
        // Device files: "major, minor" instead of a size.
        None
    };
    if fields
        .first()
        .is_some_and(|first| first.chars().all(|c| c.is_ascii_digit()))
    {
        fields.remove(0);
    }
    if size_token.ends_with(',') || fields.last().is_some_and(|field| field.ends_with(',')) {
        fields.retain(|field| !field.ends_with(','));
    }
    let owner = fields.first().map(|value| (*value).to_owned());
    let group = fields.get(1).map(|value| (*value).to_owned());
    let mtime = parse_ls_time(
        token(date_at)?,
        token(date_at + 1)?,
        token(date_at + 2)?,
        now,
    );
    let (_, after_date) = *spans.get(date_at + 2)?;
    let raw_name = &line[after_date..];
    let raw_name = raw_name.strip_prefix(' ').unwrap_or(raw_name);
    if raw_name.is_empty() {
        return None;
    }
    let (name, link_target) = match (kind, raw_name.split_once(" -> ")) {
        (EntryKind::Symlink, Some((name, target))) => (name, Some(target.to_owned())),
        _ => (raw_name, None),
    };
    // Some servers (ProFTPD) echo the full path when a single file is listed.
    let name = path::file_name(name).to_owned();
    Some(Listed {
        is_dot: name == "." || name == "..",
        name,
        kind,
        size: if kind == EntryKind::File { size } else { None },
        mtime,
        mode: Some(mode),
        owner,
        group,
        link_target,
    })
}

fn is_time_or_year(value: &str) -> bool {
    match value.split_once(':') {
        Some((hours, minutes)) => {
            hours.len() <= 2
                && minutes.len() == 2
                && hours
                    .chars()
                    .chain(minutes.chars())
                    .all(|c| c.is_ascii_digit())
        }
        None => value.len() == 4 && value.chars().all(|c| c.is_ascii_digit()),
    }
}

/// `ls` dates: `Nov  8 16:08` (within the last six months, year omitted) or `Nov  8  2025`.
/// Server-local time is unknown, so it is read as UTC.
fn parse_ls_time(mon: &str, day: &str, time_or_year: &str, now: DateTime<Utc>) -> Option<i64> {
    let month = month(mon)?;
    let day: u32 = day.parse().ok()?;
    let (year, hour, minute) = match time_or_year.split_once(':') {
        Some((hour, minute)) => (now.year(), hour.parse().ok()?, minute.parse().ok()?),
        None => (time_or_year.parse().ok()?, 0, 0),
    };
    let at = |year: i32| {
        NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_opt(hour, minute, 0))
            .map(|time| Utc.from_utc_datetime(&time))
    };
    let mut time = at(year)?;
    // A date without a year is in the past: if it would be in the future, it is last year's.
    if !time_or_year.contains(':') {
        return Some(time.timestamp());
    }
    if time > now + chrono::Duration::days(1) {
        time = at(year - 1)?;
    }
    Some(time.timestamp())
}

fn parse_with_suppaftp(line: &str) -> Option<Listed> {
    // Not `File::try_from`: its last resort, the MLST parser, reads "total 48" as an entry.
    let file = suppaftp::list::ListParser::parse_posix(line)
        .or_else(|_| suppaftp::list::ListParser::parse_dos(line))
        .ok()?;
    let name = path::file_name(file.name()).to_owned();
    let kind = if file.is_directory() {
        EntryKind::Dir
    } else if file.is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::File
    };
    let mtime = file
        .modified()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok());
    Some(Listed {
        is_dot: name == "." || name == "..",
        link_target: file
            .symlink()
            .map(|target| target.to_string_lossy().into_owned()),
        name,
        kind,
        size: (kind == EntryKind::File).then(|| file.size() as u64),
        mtime,
        mode: None,
        owner: None,
        group: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 8, 18, 0, 0).unwrap()
    }

    #[test]
    fn parses_proftpd_mlsd_facts() {
        let line = "modify=20261108160029;perm=adfrw;size=574;type=file;unique=10302U23204F2;\
                    UNIX.group=1000;UNIX.groupname=gem;UNIX.mode=0755;UNIX.owner=1000;\
                    UNIX.ownername=gem; deploy.sh";
        let listed = parse_mlsx(line).unwrap();
        assert_eq!(listed.name, "deploy.sh");
        assert_eq!(listed.kind, EntryKind::File);
        assert_eq!(listed.size, Some(574));
        assert_eq!(listed.mode, Some(0o755));
        assert_eq!(listed.owner.as_deref(), Some("gem"));
        assert_eq!(listed.group.as_deref(), Some("gem"));
        assert_eq!(
            listed.mtime,
            Some(
                Utc.with_ymd_and_hms(2026, 11, 8, 16, 0, 29)
                    .unwrap()
                    .timestamp()
            )
        );
        assert!(!listed.is_dot);
    }

    #[test]
    fn mlsd_types_names_and_special_bits() {
        let dir = parse_mlsx("type=dir;UNIX.mode=02755;modify=20261108160229; sub dir").unwrap();
        assert_eq!(dir.kind, EntryKind::Dir);
        assert_eq!(dir.name, "sub dir");
        assert_eq!(dir.mode, Some(0o2755));
        assert!(
            parse_mlsx("type=cdir;modify=20261108161307; .")
                .unwrap()
                .is_dot
        );
        assert!(parse_mlsx("type=pdir; ..").unwrap().is_dot);
        let link = parse_mlsx("type=OS.unix=symlink;size=7;UNIX.mode=0777; link-to-dir").unwrap();
        assert_eq!(link.kind, EntryKind::Symlink);
        assert_eq!(link.size, None);
        let slink = parse_mlsx("type=OS.unix=slink:/home/gem/Samples; x").unwrap();
        assert_eq!(slink.link_target.as_deref(), Some("/home/gem/Samples"));
        // Numeric ids when names are missing; fractional seconds; no facts at all.
        let ids = parse_mlsx("Type=file;Modify=20261108160000.123;UNIX.owner=0; a").unwrap();
        assert_eq!(ids.owner.as_deref(), Some("0"));
        assert!(ids.mtime.is_some());
        assert_eq!(parse_mlsx(" bare").unwrap().name, "bare");
        assert_eq!(parse_mlsx("type=file;"), None);
    }

    #[test]
    fn parses_vsftpd_and_proftpd_list_lines() {
        let file = parse_list_line(
            "-rwxr-xr-x    1 gem      gem           574 Nov 08 16:00 deploy.sh",
            now(),
        )
        .unwrap();
        assert_eq!(file.kind, EntryKind::File);
        assert_eq!(file.mode, Some(0o755));
        assert_eq!(file.size, Some(574));
        assert_eq!(file.owner.as_deref(), Some("gem"));
        assert_eq!(file.group.as_deref(), Some("gem"));
        assert_eq!(
            file.mtime,
            Some(
                Utc.with_ymd_and_hms(2026, 11, 8, 16, 0, 0)
                    .unwrap()
                    .timestamp()
            )
        );
        let spaced = parse_list_line(
            "-rw-r--r--    1 1000     1000            6 Nov 08 16:08 with  two  spaces.txt",
            now(),
        )
        .unwrap();
        assert_eq!(spaced.name, "with  two  spaces.txt");
        let unicode = parse_list_line(
            "-rw-r--r--    1 1000     1000            8 Nov 08 16:08 ação ünïcödé.txt",
            now(),
        )
        .unwrap();
        assert_eq!(unicode.name, "ação ünïcödé.txt");
        let link = parse_list_line(
            "lrwxrwxrwx    1 gem      gem            17 Nov 08 16:08 link-to-file -> samples/index.php",
            now(),
        )
        .unwrap();
        assert_eq!(link.kind, EntryKind::Symlink);
        assert_eq!(link.name, "link-to-file");
        assert_eq!(link.link_target.as_deref(), Some("samples/index.php"));
        assert!(
            parse_list_line(
                "drwxr-xr-x   10 gem      gem          4096 Nov  8 16:13 .",
                now()
            )
            .unwrap()
            .is_dot
        );
        // ProFTPD echoes the full path when one file is listed.
        let echoed = parse_list_line(
            "-rw-r--r--   1 gem      gem             6 Nov  8 16:13 /home/gem/names/with space.txt",
            now(),
        )
        .unwrap();
        assert_eq!(echoed.name, "with space.txt");
    }

    #[test]
    fn list_lines_keep_special_bits_and_handle_variants() {
        let sticky =
            parse_list_line("drwxrwxrwt 2 root root 4096 Jan  3  2025 tmp", now()).unwrap();
        assert_eq!(sticky.mode, Some(0o1777));
        assert_eq!(
            sticky.mtime,
            Some(
                Utc.with_ymd_and_hms(2025, 1, 3, 0, 0, 0)
                    .unwrap()
                    .timestamp()
            )
        );
        let setgid = parse_list_line("drwxr-sr-x 2 a b 4096 Nov 08 16:02 logs", now()).unwrap();
        assert_eq!(setgid.mode, Some(0o2755));
        let setuid = parse_list_line("-rwSr--r-- 1 a b 1 Nov 08 16:02 x", now()).unwrap();
        assert_eq!(setuid.mode, Some(0o4644));
        // No group column; ACL marker.
        let no_group =
            parse_list_line("-rw-r--r--+ 1 owner 1234 Nov 8 10:00 a.txt", now()).unwrap();
        assert_eq!(no_group.owner.as_deref(), Some("owner"));
        assert_eq!(no_group.group, None);
        assert_eq!(no_group.size, Some(1234));
        // A date in "the future" without a year belongs to last year.
        let december = parse_list_line("-rw-r--r-- 1 a b 1 Dec 24 10:00 x", now()).unwrap();
        assert_eq!(
            december.mtime,
            Some(
                Utc.with_ymd_and_hms(2025, 12, 24, 10, 0, 0)
                    .unwrap()
                    .timestamp()
            )
        );
        let device =
            parse_list_line("crw-rw-rw- 1 root root 1, 3 Nov 8 10:00 null", now()).unwrap();
        assert_eq!(device.kind, EntryKind::Other);
        assert_eq!(device.owner.as_deref(), Some("root"));
        assert_eq!(parse_list_line("total 48", now()), None);
        assert_eq!(parse_list_line("", now()), None);
    }

    #[test]
    fn dos_lines_go_through_suppaftp() {
        let dir = parse_list_line("04-27-20  12:05PM       <DIR>          docs", now()).unwrap();
        assert_eq!(dir.kind, EntryKind::Dir);
        assert_eq!(dir.name, "docs");
        let file =
            parse_list_line("04-27-20  12:05PM                 1234 a file.txt", now()).unwrap();
        assert_eq!(file.kind, EntryKind::File);
        assert_eq!(file.size, Some(1234));
        assert_eq!(file.name, "a file.txt");
        assert_eq!(file.mode, None);
    }
}
