//! Adds or changes a saved connection.
//!
//! The fields shown follow the kind: an object store has no port and a bucket, SFTP has a port
//! and no bucket. Everything is handed to the list as one set of fields, so adding a
//! destination later means adding fields here and nothing else.

use std::rc::Rc;

use adw::prelude::*;

use crate::connections::{self, CREDENTIALS, Fields, KINDS};
use crate::window::Window;

pub fn compose(window: &Rc<Window>) {
    Editor::open(window, None);
}

/// Opens the editor on a connection that already exists, showing what it holds. Opening blank
/// and saving would quietly replace the whole thing with an empty one.
pub fn amend(window: &Rc<Window>, id: &str) {
    if let Some(saved) = connections::list().details(id) {
        Editor::open(window, Some(saved));
    }
}

struct Editor {
    dialog: adw::Dialog,
    editing: String,
    name: adw::EntryRow,
    kind: adw::ComboRow,
    host: adw::EntryRow,
    port: adw::EntryRow,
    credential: adw::ComboRow,
    key: adw::EntryRow,
    nodelay: adw::SwitchRow,
    url: adw::EntryRow,
    username: adw::EntryRow,
    bucket: adw::EntryRow,
    region: adw::EntryRow,
    endpoint: adw::EntryRow,
    save: gtk::Button,
}

impl Editor {
    fn open(window: &Rc<Window>, saved: Option<Fields>) {
        let editing = saved.as_ref().map(|saved| saved.text("id")).unwrap_or_default();

        let name = adw::EntryRow::builder().title("Name").build();
        let kind = adw::ComboRow::builder().title("Kind").build();
        kind.set_model(Some(&gtk::StringList::new(&KINDS.map(|(_, label)| label))));

        let host = adw::EntryRow::builder().title("Host").build();
        let port = adw::EntryRow::builder().title("Port").input_purpose(gtk::InputPurpose::Digits).build();

        let credential = adw::ComboRow::builder().title("Sign in with").build();
        credential.set_model(Some(&gtk::StringList::new(&CREDENTIALS.map(|(_, label)| label))));

        let key = adw::EntryRow::builder().title("Key file").build();

        // On by default, because SFTP is small requests and acknowledgements, and holding
        // those back to bundle them is time spent waiting. Here for the odd link where the
        // packet count is what hurts.
        let nodelay = adw::SwitchRow::builder()
            .title("Use TCP nodelay")
            .subtitle("Faster uploads, but can use more bandwidth")
            .active(true)
            .build();
        let url = adw::EntryRow::builder().title("URL").build();
        let username = adw::EntryRow::builder().title("Username").build();
        let bucket = adw::EntryRow::builder().title("Bucket").build();
        let region = adw::EntryRow::builder().title("Region").build();
        let endpoint = adw::EntryRow::builder().title("Endpoint (only if it is not the usual one)").build();

        let about = adw::PreferencesGroup::new();
        about.add(&name);
        about.add(&kind);

        let where_ = adw::PreferencesGroup::new();

        // In the order each kind reads best: host, port and who you are before how you prove
        // it; the address before the account for WebDAV; the switch last.
        for row in [&host, &port, &url, &username] {
            where_.add(row);
        }

        where_.add(&credential);

        for row in [&key, &bucket, &region, &endpoint] {
            where_.add(row);
        }

        where_.add(&nodelay);

        let page = adw::PreferencesPage::new();
        page.add(&about);
        page.add(&where_);

        let cancel = gtk::Button::with_label("Cancel");
        let save = gtk::Button::with_label("Save");
        save.add_css_class("suggested-action");

        let header = adw::HeaderBar::new();
        header.set_show_end_title_buttons(false);
        header.set_show_start_title_buttons(false);
        header.pack_start(&cancel);
        header.pack_end(&save);

        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.set_content(Some(&page));

        let dialog = adw::Dialog::builder()
            .title(match editing.is_empty() {
                true => "New connection",
                false => "Edit connection",
            })
            .content_width(460)
            .child(&content)
            .build();

        // Connecting only asks for a credential when none is saved, so without this a
        // mistyped access key could only be corrected from the desktop's keyring.
        if !editing.is_empty() {
            let more = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            more.set_halign(gtk::Align::End);

            if connections::list().needs_credentials(&editing) {
                let change = gtk::Button::with_label("Change credentials");
                change.connect_clicked({
                    let (weak, id) = (Rc::downgrade(window), editing.clone());
                    let dialog = dialog.clone();

                    move |_| {
                        if let Some(window) = weak.upgrade() {
                            window.change_credentials(&id);
                        }

                        dialog.close();
                    }
                });
                more.append(&change);
            }

            let delete = gtk::Button::with_label("Delete");
            delete.add_css_class("destructive-action");
            delete.connect_clicked({
                let id = editing.clone();
                let dialog = dialog.clone();

                move |_| {
                    connections::list().forget(&id);
                    dialog.close();
                }
            });
            more.append(&delete);

            let group = adw::PreferencesGroup::new();
            group.add(&more);
            page.add(&group);
        }

        let editor = Rc::new(Self {
            dialog,
            editing,
            name,
            kind,
            host,
            port,
            credential,
            key,
            nodelay,
            url,
            username,
            bucket,
            region,
            endpoint,
            save,
        });

        if let Some(saved) = saved {
            editor.fill(&saved);
        }

        cancel.connect_clicked({
            let dialog = editor.dialog.clone();

            move |_| {
                dialog.close();
            }
        });

        editor.save.connect_clicked({
            let editor = Rc::clone(&editor);

            move |_| {
                connections::list().save(editor.fields());
                editor.dialog.close();
            }
        });

        editor.name.connect_changed({
            let editor = Rc::clone(&editor);

            move |_| editor.refresh()
        });

        editor.kind.connect_selected_notify({
            let editor = Rc::clone(&editor);

            move |_| editor.refresh()
        });

        editor.credential.connect_selected_notify({
            let editor = Rc::clone(&editor);

            move |_| editor.refresh()
        });

        editor.refresh();
        editor.dialog.present(Some(&window.gtk));
        editor.name.grab_focus();
    }

    fn fill(&self, saved: &Fields) {
        self.name.set_text(&saved.text("name"));
        self.host.set_text(&saved.text("host"));
        self.port.set_text(&saved.text("port"));
        self.username.set_text(&saved.text("username"));
        self.bucket.set_text(&saved.text("bucket"));
        self.region.set_text(&saved.text("region"));
        self.endpoint.set_text(&saved.text("endpoint"));
        self.url.set_text(&saved.text("url"));
        self.key.set_text(&saved.text("key"));
        self.nodelay.set_active(saved.text("nodelay") != "false");

        let kind = saved.text("kind");
        let credential = saved.text("credential");

        self.kind
            .set_selected(KINDS.iter().position(|(key, _)| *key == kind).unwrap_or(0) as u32);
        self.credential.set_selected(
            CREDENTIALS.iter().position(|(key, _)| *key == credential).unwrap_or(0) as u32,
        );
    }

    fn kind(&self) -> &'static str {
        KINDS[self.kind.selected() as usize].0
    }

    fn credential(&self) -> &'static str {
        CREDENTIALS[self.credential.selected() as usize].0
    }

    /// Shows the fields the chosen kind needs and hides the rest.
    fn refresh(&self) {
        let kind = self.kind();
        let storage = matches!(kind, "s3" | "r2" | "b2");
        let azure = kind == "azure";
        let host_based = matches!(kind, "sftp" | "ftp");

        self.host.set_visible(host_based);
        self.port.set_visible(host_based);
        self.port.set_title(match kind {
            "ftp" => "Port (21)",
            _ => "Port (22)",
        });
        self.credential.set_visible(kind == "sftp");
        self.key.set_visible(kind == "sftp" && self.credential() == "key");
        self.nodelay.set_visible(kind == "sftp");
        self.url.set_visible(kind == "webdav");
        self.username.set_visible(!storage);
        self.username.set_title(match azure {
            true => "Account",
            false => "Username",
        });
        self.bucket.set_visible(storage || azure);
        self.bucket.set_title(match azure {
            true => "Container",
            false => "Bucket",
        });
        self.region.set_visible(storage);
        self.region.set_title(match kind {
            "r2" => "Account id",
            _ => "Region",
        });
        self.endpoint.set_visible(storage || azure);

        self.save.set_sensitive(!self.name.text().trim().is_empty());
    }

    fn fields(&self) -> Fields {
        let mut fields = Fields::default();

        fields.put("id", self.editing.clone());
        fields.put("name", self.name.text().trim().to_owned());
        fields.put("kind", self.kind().to_owned());
        fields.put("host", self.host.text().trim().to_owned());
        fields.put("port", self.port.text().trim().to_owned());
        fields.put("username", self.username.text().trim().to_owned());
        fields.put("bucket", self.bucket.text().trim().to_owned());
        fields.put("region", self.region.text().trim().to_owned());
        fields.put("endpoint", self.endpoint.text().trim().to_owned());
        fields.put("url", self.url.text().trim().to_owned());
        fields.put("credential", self.credential().to_owned());
        fields.put("key", self.key.text().trim().to_owned());
        fields.put("nodelay", self.nodelay.is_active().to_string());

        fields
    }
}
