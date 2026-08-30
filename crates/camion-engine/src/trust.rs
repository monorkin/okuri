use async_trait::async_trait;
use camion_providers::{HostKey, HostTrust, Trust};

use crate::event::{Event, Prompt, Question};
use crate::known_hosts::{KnownHosts, Verdict};
use crate::Emitter;

/// Answers "is this the server we meant?" by consulting `known_hosts` first and the person at
/// the keyboard second.
pub struct PromptingTrust {
    known_hosts: KnownHosts,
    emit: Emitter,
}

impl PromptingTrust {
    pub fn new(known_hosts: KnownHosts, emit: Emitter) -> Self {
        Self { known_hosts, emit }
    }

    async fn ask(&self, question: Question) -> bool {
        let (prompt, answer) = Prompt::new(question);
        (self.emit)(Event::Ask(prompt));

        answer.await.map(|answer| answer.is_accepted()).unwrap_or(false)
    }
}

#[async_trait]
impl HostTrust for PromptingTrust {
    async fn verify(&self, key: &HostKey) -> Trust {
        match self.known_hosts.verdict(key) {
            Verdict::Known => Trust::Known,

            Verdict::Unknown => {
                let accepted = self
                    .ask(Question::UnknownHostKey {
                        host: key.host.clone(),
                        algorithm: key.algorithm.clone(),
                        fingerprint: key.fingerprint.clone(),
                    })
                    .await;

                if accepted {
                    // Written to the real `known_hosts`, so `ssh` from a terminal agrees with
                    // Camion about what has been trusted, and neither asks twice.
                    let _ = self.known_hosts.remember(key);
                    Trust::Accepted
                } else {
                    Trust::Rejected
                }
            }

            // A key that changed is either a rebuilt server or somebody in the middle, and
            // Camion cannot tell which. Accepting lets this one connection through and
            // deliberately does not rewrite the file: replacing a trusted key is a decision for
            // `ssh-keygen -R`, made deliberately, not a side effect of wanting to see a folder.
            Verdict::Changed => {
                let accepted = self
                    .ask(Question::ChangedHostKey {
                        host: key.host.clone(),
                        algorithm: key.algorithm.clone(),
                        fingerprint: key.fingerprint.clone(),
                    })
                    .await;

                if accepted {
                    Trust::Accepted
                } else {
                    Trust::Rejected
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Answer;
    use std::sync::Arc;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyDataHere0000000000000000";

    fn host_key(public_key: &str) -> HostKey {
        HostKey {
            host: "example.com".to_owned(),
            port: 22,
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "SHA256:whatever".to_owned(),
            public_key: public_key.to_owned(),
        }
    }

    fn answering(answer: Answer) -> Emitter {
        Arc::new(move |event| {
            if let Event::Ask(prompt) = event {
                prompt.answer(answer.clone());
            }
        })
    }

    #[tokio::test]
    async fn a_key_already_on_file_is_trusted_without_asking() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("known_hosts");
        std::fs::write(&path, format!("example.com {KEY}\n")).unwrap();

        let asked = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = Arc::clone(&asked);
        let emit: Emitter = Arc::new(move |event| {
            if matches!(event, Event::Ask(_)) {
                watcher.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        });

        let trust = PromptingTrust::new(KnownHosts::new(path), emit);

        assert_eq!(trust.verify(&host_key(KEY)).await, Trust::Known);
        assert!(!asked.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn accepting_a_new_key_writes_it_to_known_hosts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("known_hosts");

        let trust = PromptingTrust::new(KnownHosts::new(&path), answering(Answer::Accept));

        assert_eq!(trust.verify(&host_key(KEY)).await, Trust::Accepted);
        assert_eq!(KnownHosts::new(&path).verdict(&host_key(KEY)), Verdict::Known);
    }

    #[tokio::test]
    async fn declining_a_new_key_writes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("known_hosts");

        let trust = PromptingTrust::new(KnownHosts::new(&path), answering(Answer::Decline));

        assert_eq!(trust.verify(&host_key(KEY)).await, Trust::Rejected);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn accepting_a_changed_key_does_not_overwrite_the_one_on_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("known_hosts");
        std::fs::write(&path, format!("example.com {KEY}\n")).unwrap();

        let different = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDifferentKeyData00000000000000000";
        let trust = PromptingTrust::new(KnownHosts::new(&path), answering(Answer::Accept));

        assert_eq!(trust.verify(&host_key(different)).await, Trust::Accepted);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("example.com {KEY}\n")
        );
    }

    #[tokio::test]
    async fn a_prompt_nobody_answers_rejects_the_connection() {
        let directory = tempfile::tempdir().unwrap();
        let emit: Emitter = Arc::new(|_event| {});

        let trust = PromptingTrust::new(KnownHosts::new(directory.path().join("known_hosts")), emit);

        assert_eq!(trust.verify(&host_key(KEY)).await, Trust::Rejected);
    }
}
