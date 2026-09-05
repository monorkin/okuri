//! The small dialogs: a question from the engine, a confirmation, a name, the columns.
//!
//! All of them are Adwaita's own, so they are drawn in the palette like everything else and
//! behave the way every other dialog on the desktop does.

use adw::prelude::*;
use okuri_engine::Question;

/// A question from the engine, in the words the dialog shows.
///
/// One shape for all of them: an unknown host key, a changed one, a password, a passphrase.
/// They differ only in wording and in whether they want typing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    pub title: String,
    pub body: String,
    pub detail: String,
    /// The third choice, when the question has one. Empty when it does not.
    pub alternative: String,
    pub accept: String,
    pub wants_text: bool,
    pub wants_pair: bool,
    pub first_label: String,
    pub second_label: String,
    pub is_secret: bool,
    pub is_grave: bool,
}

impl Default for Asked {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
            detail: String::new(),
            alternative: String::new(),
            accept: "Continue".to_owned(),
            wants_text: false,
            wants_pair: false,
            first_label: String::new(),
            second_label: String::new(),
            is_secret: false,
            is_grave: false,
        }
    }
}

pub fn describe(question: &Question) -> Asked {
    match question {
        Question::UnknownHostKey { host, algorithm, fingerprint } => Asked {
            title: format!("{host} is new"),
            body: format!(
                "Okuri has never connected to {host} before. Check the {algorithm} fingerprint \
                 against the one the server's administrator gave you."
            ),
            detail: fingerprint.clone(),
            accept: "Trust and connect".to_owned(),
            ..Asked::default()
        },

        Question::ChangedHostKey { host, algorithm, fingerprint } => Asked {
            title: format!("{host} is not the machine it was"),
            body: format!(
                "The {algorithm} key {host} presented is not the one on file. This happens when a \
                 server is rebuilt — and it is also what an eavesdropper looks like. Do not \
                 continue unless you know the server changed."
            ),
            detail: fingerprint.clone(),
            accept: "Connect anyway".to_owned(),
            is_grave: true,
            ..Asked::default()
        },

        Question::Password { connection } => Asked {
            title: format!("Password for {connection}"),
            accept: "Connect".to_owned(),
            wants_text: true,
            is_secret: true,
            ..Asked::default()
        },

        Question::KeyPair { connection } => Asked {
            title: format!("Keys for {connection}"),
            body: "This kind of storage signs every request with an access key and its secret."
                .to_owned(),
            accept: "Connect".to_owned(),
            wants_pair: true,
            first_label: "Access key".to_owned(),
            second_label: "Secret".to_owned(),
            is_secret: true,
            ..Asked::default()
        },

        Question::KeyPassphrase { path } => Asked {
            title: "Unlock your SSH key".to_owned(),
            body: format!("{path} is encrypted."),
            accept: "Unlock".to_owned(),
            wants_text: true,
            is_secret: true,
            ..Asked::default()
        },

        Question::Passphrase => Asked {
            title: "Unlock your saved passwords".to_owned(),
            body: "Okuri keeps your passwords in an encrypted file on this machine.".to_owned(),
            accept: "Unlock".to_owned(),
            wants_text: true,
            is_secret: true,
            ..Asked::default()
        },

        Question::Overwrite { name } => Asked {
            title: format!("Replace {name}?"),
            body: format!("{name} is already there."),
            accept: "Replace".to_owned(),
            alternative: "Keep both".to_owned(),
            ..Asked::default()
        },
    }
}

/// What came back from a question.
pub enum Reply {
    Declined,
    Accepted { first: String, second: String },
    Alternative,
}

/// Puts a question on screen and hands back whatever is chosen.
///
/// Closing the dialog any other way declines it, which is the answer that leaves nothing
/// waiting: a connection that was refused rather than one that hangs.
pub fn ask(parent: &impl IsA<gtk::Widget>, asked: &Asked, answered: impl Fn(Reply) + 'static) {
    let body = Some(asked.body.as_str()).filter(|body| !body.is_empty());
    let dialog = adw::AlertDialog::new(Some(&asked.title), body);

    let extra = gtk::Box::new(gtk::Orientation::Vertical, 10);

    if !asked.detail.is_empty() {
        let fingerprint = gtk::Label::new(Some(&asked.detail));
        fingerprint.set_wrap(true);
        fingerprint.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        fingerprint.set_justify(gtk::Justification::Center);
        fingerprint.set_selectable(true);
        fingerprint.add_css_class("okuri-detail");
        extra.append(&fingerprint);
    }

    // An access key is not a secret and is easier to check when you can read it.
    let first = field(&asked.first_label, asked.is_secret && !asked.wants_pair);
    let second = field(&asked.second_label, asked.is_secret);

    if asked.wants_text || asked.wants_pair {
        extra.append(&first.row);
    }

    if asked.wants_pair {
        extra.append(&second.row);
    }

    if extra.first_child().is_some() {
        dialog.set_extra_child(Some(&extra));
    }

    dialog.add_response("cancel", "Cancel");

    // Only some questions have a third answer — replacing a file can also mean keeping both —
    // so this is here when there is one and gone when there is not.
    if !asked.alternative.is_empty() {
        dialog.add_response("alternative", &asked.alternative);
    }

    dialog.add_response("accept", &asked.accept);
    dialog.set_response_appearance(
        "accept",
        match asked.is_grave {
            true => adw::ResponseAppearance::Destructive,
            false => adw::ResponseAppearance::Suggested,
        },
    );
    dialog.set_default_response(Some("accept"));
    dialog.set_close_response("cancel");

    if asked.is_grave {
        dialog.add_css_class("okuri-grave");
    }

    let typing = asked.wants_text || asked.wants_pair;
    let focus = first.entry.clone();

    dialog.connect_response(None, move |_, response| {
        answered(match response {
            "accept" => Reply::Accepted { first: first.text(), second: second.text() },
            "alternative" => Reply::Alternative,
            _ => Reply::Declined,
        });
    });

    dialog.present(Some(parent));

    if typing {
        focus.grab_focus();
    }
}

/// A labelled entry, or a labelled password entry. Enter accepts the dialog either way.
struct Field {
    row: gtk::Box,
    entry: gtk::Widget,
}

impl Field {
    fn text(&self) -> String {
        self.entry
            .downcast_ref::<gtk::Editable>()
            .map(|editable| editable.text().to_string())
            .unwrap_or_default()
    }
}

fn field(label: &str, secret: bool) -> Field {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);

    if !label.is_empty() {
        let caption = gtk::Label::new(Some(label));
        caption.set_xalign(0.0);
        caption.add_css_class("okuri-muted");
        caption.add_css_class("okuri-small");
        row.append(&caption);
    }

    let entry: gtk::Widget = match secret {
        true => {
            let entry = gtk::PasswordEntry::new();
            entry.set_show_peek_icon(true);
            entry.set_activates_default(true);
            entry.upcast()
        }
        false => {
            let entry = gtk::Entry::new();
            entry.set_activates_default(true);
            entry.upcast()
        }
    };

    row.append(&entry);

    Field { row, entry }
}

/// Asks before something that cannot be undone.
///
/// There is no trash on the other end of a connection: a deleted file is gone, and the only
/// chance to say otherwise is here.
pub fn confirm(
    parent: &impl IsA<gtk::Widget>,
    question: &str,
    detail: &str,
    accept: &str,
    confirmed: impl Fn() + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(question), Some(detail));

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("accept", accept);
    dialog.set_response_appearance("accept", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    dialog.connect_response(Some("accept"), move |_, _| confirmed());
    dialog.present(Some(parent));
}

/// Asks for one name. Used for new folders and for renaming.
pub fn name(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    placeholder: &str,
    accept: &str,
    warning: &str,
    initial: &str,
    named: impl Fn(String) + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), None);

    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some(placeholder));
    entry.set_text(initial);
    entry.set_activates_default(true);

    let extra = gtk::Box::new(gtk::Orientation::Vertical, 8);
    extra.append(&entry);

    if !warning.is_empty() {
        let caution = gtk::Label::new(Some(warning));
        caution.set_wrap(true);
        caution.set_xalign(0.0);
        caution.add_css_class("okuri-warning");
        caution.add_css_class("okuri-small");
        extra.append(&caution);
    }

    dialog.set_extra_child(Some(&extra));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("accept", accept);
    dialog.set_response_appearance("accept", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("accept"));
    dialog.set_close_response("cancel");
    dialog.set_response_enabled("accept", !initial.trim().is_empty());

    entry.connect_changed({
        let dialog = dialog.downgrade();

        move |entry| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.set_response_enabled("accept", !entry.text().trim().is_empty());
            }
        }
    });

    dialog.connect_response(Some("accept"), {
        let entry = entry.clone();

        move |_, _| {
            let name = entry.text().trim().to_owned();

            if !name.is_empty() {
                named(name);
            }
        }
    });

    dialog.present(Some(parent));
    entry.grab_focus();
    entry.select_region(0, -1);
}

/// Which columns the list shows.
pub fn columns(parent: &impl IsA<gtk::Widget>) {
    let group = adw::PreferencesGroup::new();

    // Name is not a switch. A file list without names is not a file list, so it is shown as
    // fixed rather than as something you can turn off and regret.
    let always = adw::ActionRow::builder().title("Name").subtitle("always").build();
    group.add(&always);

    let settings = crate::view::current();

    for (label, column, shown) in [
        ("Size", "size", settings.columns.size),
        ("Type", "kind", settings.columns.kind),
        ("Modified", "modified", settings.columns.modified),
        ("Permissions", "permissions", settings.columns.permissions),
    ] {
        let row = adw::SwitchRow::builder().title(label).active(shown).build();

        row.connect_active_notify(move |row| {
            let visible = row.is_active();

            crate::view::update(|settings| settings.columns.set(column, visible));
        });

        group.add(&row);
    }

    let page = adw::PreferencesPage::new();
    page.add(&group);

    let content = adw::ToolbarView::new();
    content.add_top_bar(&adw::HeaderBar::new());
    content.set_content(Some(&page));

    let dialog = adw::Dialog::builder()
        .title("Visible columns")
        .content_width(360)
        .child(&content)
        .build();

    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_host_key_is_worded_as_the_danger_it_is() {
        let asked = describe(&Question::ChangedHostKey {
            host: "files.example.com".to_owned(),
            algorithm: "ed25519".to_owned(),
            fingerprint: "SHA256:abc".to_owned(),
        });

        assert!(asked.is_grave);
        assert_eq!(asked.detail, "SHA256:abc");
        assert_eq!(asked.accept, "Connect anyway");
        assert!(!asked.wants_text);
    }

    #[test]
    fn a_password_is_typed_and_hidden() {
        let asked = describe(&Question::Password { connection: "Production".to_owned() });

        assert_eq!(asked.title, "Password for Production");
        assert!(asked.wants_text);
        assert!(asked.is_secret);
        assert!(!asked.wants_pair);
    }

    #[test]
    fn a_key_pair_asks_for_two_things_and_names_both() {
        let asked = describe(&Question::KeyPair { connection: "Assets".to_owned() });

        assert!(asked.wants_pair);
        assert_eq!(asked.first_label, "Access key");
        assert_eq!(asked.second_label, "Secret");
    }

    #[test]
    fn replacing_a_file_offers_to_keep_both() {
        let asked = describe(&Question::Overwrite { name: "notes.txt".to_owned() });

        assert_eq!(asked.alternative, "Keep both");
        assert_eq!(asked.accept, "Replace");
    }
}
