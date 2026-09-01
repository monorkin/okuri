/// What a destination needs to prove who we are, once it has been fetched from the store.
///
/// Kept deliberately small: adapters take one of these and never learn where it came from, so
/// the keyring, the encrypted file, and a test fixture are interchangeable.
#[derive(Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Secret {
    /// Nothing to supply — an SSH agent, or an anonymous endpoint.
    #[default]
    None,
    /// A password, a passphrase, or an account key.
    Password(String),
    /// An identifier and its secret, the shape S3 and its lookalikes want.
    KeyPair { id: String, secret: String },
}

impl Secret {
    pub fn password(&self) -> Option<&str> {
        match self {
            Self::Password(password) => Some(password),
            _ => None,
        }
    }

    pub fn key_pair(&self) -> Option<(&str, &str)> {
        match self {
            Self::KeyPair { id, secret } => Some((id, secret)),
            _ => None,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// Never print a secret, however convenient it would be while debugging. `Debug` deliberately
/// says the same thing as `Display`, so a secret cannot reach a log through a derived format.
impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let described = match self {
            Self::None => "none",
            Self::Password(_) => "a password",
            Self::KeyPair { .. } => "a key pair",
        };

        formatter.write_str(described)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_do_not_leak_through_formatting() {
        let secret = Secret::Password("hunter2".to_owned());

        assert_eq!(secret.to_string(), "a password");
        assert!(!format!("{secret:?}").contains("hunter2"));
    }

    #[test]
    fn each_shape_answers_only_for_itself() {
        let password = Secret::Password("hunter2".to_owned());
        let pair = Secret::KeyPair { id: "AKIA".to_owned(), secret: "shh".to_owned() };

        assert_eq!(password.password(), Some("hunter2"));
        assert_eq!(password.key_pair(), None);
        assert_eq!(pair.key_pair(), Some(("AKIA", "shh")));
        assert_eq!(pair.password(), None);
    }
}
