use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use camion_providers::HostKey;
use hmac::{Hmac, Mac};
use sha1::Sha1;

/// What `~/.ssh/known_hosts` has to say about a server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The key on file is this key. Connect without asking.
    Known,
    /// Nothing on file. Worth asking the person at the keyboard.
    Unknown,
    /// A different key is on file. Either the server was rebuilt, or someone is in the middle.
    Changed,
}

/// Camion's view of OpenSSH's `known_hosts`.
///
/// The real file, in the real format, so that trusting a server here means `ssh` trusts it too
/// and vice versa. Keeping a private trust store would mean answering the same question twice
/// and, worse, disagreeing with the tool people already check against.
pub struct KnownHosts {
    path: PathBuf,
}

impl KnownHosts {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> Option<PathBuf> {
        Some(Path::new(&std::env::var_os("HOME")?).join(".ssh/known_hosts"))
    }

    pub fn verdict(&self, key: &HostKey) -> Verdict {
        let contents = std::fs::read_to_string(&self.path).unwrap_or_default();
        let host = host_pattern(&key.host, key.port);

        let keys_on_file = contents
            .lines()
            .filter_map(parse_line)
            .filter(|line| line.matches_host(&host))
            .map(|line| line.public_key)
            .collect::<Vec<_>>();

        if keys_on_file.is_empty() {
            Verdict::Unknown
        } else if keys_on_file.contains(&key.public_key) {
            Verdict::Known
        } else {
            Verdict::Changed
        }
    }

    /// Appends a key, the way `ssh` does when you answer yes.
    pub fn remember(&self, key: &HostKey) -> std::io::Result<()> {
        if let Some(directory) = self.path.parent() {
            std::fs::create_dir_all(directory)?;
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        writeln!(file, "{} {}", host_pattern(&key.host, key.port), key.public_key)
    }
}

/// `known_hosts` writes a non-standard port as `[host]:port`. Getting this wrong would mean
/// Camion and `ssh` quietly disagreeing about what has been trusted.
pub fn host_pattern(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    }
}

struct Line<'a> {
    hosts: &'a str,
    public_key: String,
}

impl Line<'_> {
    fn matches_host(&self, host: &str) -> bool {
        self.hosts.split(',').any(|pattern| match_pattern(pattern, host))
    }
}

fn parse_line(line: &str) -> Option<Line<'_>> {
    let line = line.trim();

    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let mut fields = line.split_whitespace();
    let hosts = fields.next()?;

    // `@cert-authority` and `@revoked` lines mean something other than "this key is fine", so
    // neither is read as one.
    if hosts.starts_with('@') {
        return None;
    }

    let algorithm = fields.next()?;
    let key = fields.next()?;

    Some(Line { hosts, public_key: format!("{algorithm} {key}") })
}

fn match_pattern(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("|1|") {
        Some(hashed) => matches_hashed(hashed, host),
        None => pattern == host,
    }
}

/// OpenSSH hashes hostnames by default on many distributions, so a plain string comparison
/// would find nothing in a perfectly normal file.
fn matches_hashed(hashed: &str, host: &str) -> bool {
    let (salt, expected) = match hashed.split_once('|') {
        Some(parts) => parts,
        None => return false,
    };

    let encoding = base64::engine::general_purpose::STANDARD;
    let (Ok(salt), Ok(expected)) = (encoding.decode(salt), encoding.decode(expected)) else {
        return false;
    };

    match Hmac::<Sha1>::new_from_slice(&salt) {
        Ok(mut mac) => {
            mac.update(host.as_bytes());
            mac.verify_slice(&expected).is_ok()
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED25519: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyDataHere0000000000000000";
    const OTHER: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDifferentKeyData00000000000000000";

    fn key(host: &str, port: u16, public_key: &str) -> HostKey {
        HostKey {
            host: host.to_owned(),
            port,
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "SHA256:whatever".to_owned(),
            public_key: public_key.to_owned(),
        }
    }

    fn known_hosts_with(contents: &str) -> (tempfile::TempDir, KnownHosts) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("known_hosts");
        std::fs::write(&path, contents).unwrap();

        (directory, KnownHosts::new(path))
    }

    #[test]
    fn a_matching_key_is_known() {
        let (_directory, known_hosts) =
            known_hosts_with(&format!("example.com {ED25519}\n"));

        assert_eq!(known_hosts.verdict(&key("example.com", 22, ED25519)), Verdict::Known);
    }

    #[test]
    fn an_unlisted_host_is_unknown() {
        let (_directory, known_hosts) = known_hosts_with(&format!("example.com {ED25519}\n"));

        assert_eq!(known_hosts.verdict(&key("elsewhere.com", 22, ED25519)), Verdict::Unknown);
    }

    #[test]
    fn a_different_key_for_a_known_host_is_a_change_not_a_new_host() {
        let (_directory, known_hosts) = known_hosts_with(&format!("example.com {ED25519}\n"));

        assert_eq!(known_hosts.verdict(&key("example.com", 22, OTHER)), Verdict::Changed);
    }

    #[test]
    fn a_non_standard_port_is_bracketed_the_way_openssh_writes_it() {
        let (_directory, known_hosts) = known_hosts_with(&format!("[example.com]:2222 {ED25519}\n"));

        assert_eq!(known_hosts.verdict(&key("example.com", 2222, ED25519)), Verdict::Known);
        assert_eq!(known_hosts.verdict(&key("example.com", 22, ED25519)), Verdict::Unknown);
    }

    #[test]
    fn one_line_can_list_several_hosts() {
        let (_directory, known_hosts) =
            known_hosts_with(&format!("example.com,203.0.113.7 {ED25519}\n"));

        assert_eq!(known_hosts.verdict(&key("203.0.113.7", 22, ED25519)), Verdict::Known);
    }

    #[test]
    fn hashed_hostnames_are_matched_rather_than_ignored() {
        let mut mac = Hmac::<Sha1>::new_from_slice(b"sixteen bytes ok").unwrap();
        mac.update(b"example.com");

        let encoding = base64::engine::general_purpose::STANDARD;
        let line = format!(
            "|1|{}|{} {ED25519}\n",
            encoding.encode(b"sixteen bytes ok"),
            encoding.encode(mac.finalize().into_bytes())
        );

        let (_directory, known_hosts) = known_hosts_with(&line);

        assert_eq!(known_hosts.verdict(&key("example.com", 22, ED25519)), Verdict::Known);
        assert_eq!(known_hosts.verdict(&key("elsewhere.com", 22, ED25519)), Verdict::Unknown);
    }

    #[test]
    fn comments_markers_and_blank_lines_are_not_keys() {
        let (_directory, known_hosts) = known_hosts_with(&format!(
            "# a comment\n\n@revoked example.com {ED25519}\n@cert-authority example.com {ED25519}\n"
        ));

        assert_eq!(known_hosts.verdict(&key("example.com", 22, ED25519)), Verdict::Unknown);
    }

    #[test]
    fn remembering_a_key_makes_it_known_and_leaves_the_rest_alone() {
        let (_directory, known_hosts) = known_hosts_with("# existing file\nelsewhere.com ssh-rsa AAAA\n");
        let key = key("example.com", 2222, ED25519);

        known_hosts.remember(&key).unwrap();

        assert_eq!(known_hosts.verdict(&key), Verdict::Known);
        assert_eq!(
            known_hosts.verdict(&self::key("elsewhere.com", 22, "ssh-rsa AAAA")),
            Verdict::Known
        );
    }

    #[test]
    fn a_missing_file_means_every_host_is_new() {
        let directory = tempfile::tempdir().unwrap();
        let known_hosts = KnownHosts::new(directory.path().join("nothing/known_hosts"));
        let key = key("example.com", 22, ED25519);

        assert_eq!(known_hosts.verdict(&key), Verdict::Unknown);

        known_hosts.remember(&key).unwrap();
        assert_eq!(known_hosts.verdict(&key), Verdict::Known);
    }
}
