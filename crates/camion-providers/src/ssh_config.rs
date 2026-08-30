use std::path::PathBuf;

/// What `~/.ssh/config` says about a host.
///
/// Anyone who can reach a server with `ssh` expects to reach it from here, and for most people
/// that file is doing real work: pointing at a key, at an agent that is not the session's, at a
/// different port, or at a host whose real name is nothing like the one they type.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SshConfig {
    pub hostname: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub identity_files: Vec<PathBuf>,
    /// The socket of the agent to ask, when it is not the one in `SSH_AUTH_SOCK`. Password
    /// managers commonly run their own and point at it from here.
    pub identity_agent: Option<PathBuf>,
}

impl SshConfig {
    pub fn for_host(host: &str) -> Self {
        let Some(home) = home() else {
            return Self::default();
        };

        match std::fs::read_to_string(home.join(".ssh/config")) {
            Ok(contents) => Self::parse(&contents, host),
            Err(_) => Self::default(),
        }
    }

    /// Reads the file the way `ssh` reads it.
    ///
    /// The first value found for a keyword wins, and every `Host` block whose patterns match
    /// contributes — which is why a `Host *` block at the bottom still applies to everything
    /// above it, and why one at the top wins over the rest.
    pub fn parse(contents: &str, host: &str) -> Self {
        let mut config = Self::default();
        let mut applies = false;

        for line in contents.lines() {
            let line = line.trim();

            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let Some((keyword, value)) = split(line) else {
                continue;
            };

            if keyword.eq_ignore_ascii_case("host") {
                applies = value.split_whitespace().any(|pattern| matches(pattern, host));
                continue;
            }

            if !applies {
                continue;
            }

            match keyword.to_ascii_lowercase().as_str() {
                "hostname" => config.hostname.get_or_insert_with(|| value.to_owned()),
                "user" => config.user.get_or_insert_with(|| value.to_owned()),
                "port" => {
                    if let Ok(port) = value.parse() {
                        config.port.get_or_insert(port);
                    }

                    continue;
                }
                // Unlike the rest, every identity named is worth trying.
                "identityfile" => {
                    config.identity_files.push(expand(value));
                    continue;
                }
                "identityagent" => {
                    config.identity_agent.get_or_insert_with(|| expand(value));
                    continue;
                }
                _ => continue,
            };
        }

        config
    }
}

fn split(line: &str) -> Option<(&str, &str)> {
    // `ssh` accepts `Keyword value` and `Keyword=value` alike.
    let (keyword, value) = match line.split_once('=') {
        Some((keyword, value)) if !keyword.contains(char::is_whitespace) => (keyword, value),
        _ => line.split_once(char::is_whitespace)?,
    };

    let value = value.trim().trim_matches('"');

    match value.is_empty() {
        true => None,
        false => Some((keyword.trim(), value)),
    }
}

/// The subset of shell globbing `ssh` uses for host patterns: `*` for any run of characters,
/// `?` for exactly one, and everything else matched literally end to end.
fn matches(pattern: &str, host: &str) -> bool {
    // A negated pattern excludes rather than includes, and reading it as a match would apply
    // exactly the settings it was written to avoid.
    if pattern.starts_with('!') {
        return false;
    }

    let (pattern, host) = (pattern.as_bytes(), host.as_bytes());
    let (mut expected, mut at) = (0, 0);
    let (mut star, mut resumed) = (None, 0);

    while at < host.len() {
        let here = pattern.get(expected);

        if here == Some(&b'?') || (here.is_some() && here == host.get(at)) {
            expected += 1;
            at += 1;
        } else if here == Some(&b'*') {
            // Remember where the run began, so a later mismatch can come back and let it
            // swallow one more character.
            star = Some(expected);
            resumed = at;
            expected += 1;
        } else if let Some(star) = star {
            expected = star + 1;
            resumed += 1;
            at = resumed;
        } else {
            return false;
        }
    }

    pattern[expected..].iter().all(|character| *character == b'*')
}

fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().map(|home| home.join(rest)).unwrap_or_else(|| path.into()),
        None => PathBuf::from(path),
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reason a connection can work in a terminal and fail here: the agent is not the
    /// session's, it is one a password manager runs and points at from this file.
    #[test]
    fn an_agent_named_in_the_config_is_found() {
        let config = SshConfig::parse(
            "Host *\n\tIdentityAgent ~/.1password/agent.sock\n",
            "shire",
        );

        assert_eq!(
            config.identity_agent,
            Some(home().unwrap().join(".1password/agent.sock"))
        );
    }

    #[test]
    fn a_host_block_only_applies_to_the_hosts_it_names() {
        let contents = "\
Host shire
    HostName 203.0.113.7
    User frodo
    Port 2222

Host elsewhere
    HostName 198.51.100.1
";

        let shire = SshConfig::parse(contents, "shire");
        assert_eq!(shire.hostname.as_deref(), Some("203.0.113.7"));
        assert_eq!(shire.user.as_deref(), Some("frodo"));
        assert_eq!(shire.port, Some(2222));

        let other = SshConfig::parse(contents, "elsewhere");
        assert_eq!(other.hostname.as_deref(), Some("198.51.100.1"));
        assert_eq!(other.user, None);

        assert_eq!(SshConfig::parse(contents, "unmentioned"), SshConfig::default());
    }

    /// The first value wins, which is what makes a specific block at the top override the
    /// catch-all underneath it.
    #[test]
    fn the_first_value_for_a_keyword_is_the_one_that_counts() {
        let config = SshConfig::parse(
            "Host shire\n  Port 2222\n\nHost *\n  Port 22\n  User everyone\n",
            "shire",
        );

        assert_eq!(config.port, Some(2222));
        assert_eq!(config.user.as_deref(), Some("everyone"));
    }

    #[test]
    fn every_identity_named_is_worth_trying() {
        let config = SshConfig::parse(
            "Host *\n  IdentityFile ~/.ssh/work\n  IdentityFile /keys/spare\n",
            "shire",
        );

        assert_eq!(
            config.identity_files,
            vec![home().unwrap().join(".ssh/work"), PathBuf::from("/keys/spare")]
        );
    }

    #[test]
    fn patterns_match_the_way_ssh_matches_them() {
        assert!(matches("*", "shire"));
        assert!(matches("*.example.com", "web.example.com"));
        assert!(matches("web?.example.com", "web1.example.com"));
        assert!(!matches("web?.example.com", "web12.example.com"));
        assert!(matches("shire", "shire"));
        assert!(!matches("shire", "shire.local"));

        // A pattern written to exclude a host must not be read as including it.
        assert!(!matches("!shire", "shire"));
    }

    #[test]
    fn both_ways_of_writing_a_setting_are_read() {
        let spaced = SshConfig::parse("Host shire\n  Port 2222\n", "shire");
        let equals = SshConfig::parse("Host=shire\n  Port=2222\n", "shire");

        assert_eq!(spaced.port, Some(2222));
        assert_eq!(equals.port, Some(2222));
    }

    #[test]
    fn comments_and_blank_lines_are_not_settings() {
        let config = SshConfig::parse("# Port 9999\nHost shire\n\n  # User nobody\n  Port 22\n", "shire");

        assert_eq!(config.port, Some(22));
        assert_eq!(config.user, None);
    }
}
