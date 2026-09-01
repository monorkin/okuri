use time::OffsetDateTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    Folder,
    File,
}

impl EntryKind {
    pub fn is_folder(&self) -> bool {
        matches!(self, Self::Folder)
    }

    pub fn is_file(&self) -> bool {
        matches!(self, Self::File)
    }
}

/// Unix permission bits, for the providers that have them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Permissions(pub u32);

/// Whose permissions are being asked about. The values are the shift, in threes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    Everyone = 0,
    Group = 1,
    Owner = 2,
}

/// What may be done. The values are the bits, as every Unix has written them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Execute = 0b001,
    Write = 0b010,
    Read = 0b100,
}

impl Permissions {
    pub fn mode(&self) -> u32 {
        self.0 & 0o7777
    }

    /// Whether `who` may do `what`, for showing the mode as something other than nine
    /// characters of shorthand.
    pub fn allows(&self, who: Who, what: Access) -> bool {
        let bits = (self.mode() >> (who as u32 * 3)) & 0b111;

        bits & what as u32 != 0
    }

    /// The `rwxr-xr-x` form, which is what the list column shows.
    pub fn to_symbolic(&self) -> String {
        let mode = self.mode();

        (0..3)
            .rev()
            .flat_map(|group| {
                let bits = (mode >> (group * 3)) & 0b111;
                [
                    if bits & 0b100 != 0 { 'r' } else { '-' },
                    if bits & 0b010 != 0 { 'w' } else { '-' },
                    if bits & 0b001 != 0 { 'x' } else { '-' },
                ]
            })
            .collect()
    }
}

/// One row in the file list.
///
/// The name is a single segment, never a path — the listing's own path says where it lives.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<OffsetDateTime>,
    pub permissions: Option<Permissions>,
}

impl Entry {
    pub fn file(name: impl Into<String>, size: u64) -> Self {
        Self {
            name: name.into(),
            kind: EntryKind::File,
            size,
            modified: None,
            permissions: None,
        }
    }

    pub fn folder(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: EntryKind::Folder,
            size: 0,
            modified: None,
            permissions: None,
        }
    }

    pub fn modified_at(mut self, modified: OffsetDateTime) -> Self {
        self.modified = Some(modified);
        self
    }

    pub fn with_permissions(mut self, permissions: Permissions) -> Self {
        self.permissions = Some(permissions);
        self
    }

    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }
}

/// How a listing is sorted. Folders lead in every case, which is what every file manager does
/// and what people expect when they hit a column header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: Column,
    pub descending: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    Size,
    Modified,
    /// Groups files of a kind together, by the extension their name ends in, and orders each
    /// group by name.
    Kind,
}

impl Sort {
    pub fn by_name() -> Self {
        Self { column: Column::Name, descending: false }
    }

    pub fn apply(&self, entries: &mut [Entry]) {
        entries.sort_by(|left, right| self.order(left, right));
    }

    /// The same sort over anything that can be read as an entry, so a list of shared handles
    /// can be ordered without copying what they point at.
    pub fn apply_to<T: std::borrow::Borrow<Entry>>(&self, entries: &mut [T]) {
        entries.sort_by(|left, right| self.order(left.borrow(), right.borrow()));
    }

    fn order(&self, left: &Entry, right: &Entry) -> std::cmp::Ordering {
        let mut within_kind = self.compare(left, right);

        if self.descending {
            within_kind = within_kind.reverse();
        }

        left.kind.cmp(&right.kind).then(within_kind)
    }

    fn compare(&self, left: &Entry, right: &Entry) -> std::cmp::Ordering {
        match self.column {
            Column::Name => natural_order(&left.name, &right.name),
            Column::Size => left.size.cmp(&right.size),
            Column::Modified => left.modified.cmp(&right.modified),
            Column::Kind => extension_of(&left.name)
                .cmp(extension_of(&right.name))
                .then_with(|| natural_order(&left.name, &right.name)),
        }
    }
}

fn extension_of(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension,
        _ => "",
    }
}

/// Compares names the way a person reads them, so `file10` sorts after `file9` and case does
/// not scatter related names apart.
///
/// Case is remembered rather than acted on: it only decides names that are otherwise the same,
/// which is what keeps `file1` next to `File2` instead of grouping every capital first.
fn natural_order(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left = left.chars().peekable();
    let mut right = right.chars().peekable();
    let mut case_tiebreak = std::cmp::Ordering::Equal;

    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return case_tiebreak,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(first), Some(second)) if first.is_ascii_digit() && second.is_ascii_digit() => {
                let ordering = take_number(&mut left).cmp(&take_number(&mut right));

                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }
            (Some(first), Some(second)) => {
                left.next();
                right.next();

                let ordering = first.to_ascii_lowercase().cmp(&second.to_ascii_lowercase());

                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }

                case_tiebreak = case_tiebreak.then(first.cmp(&second));
            }
        }
    }
}

fn take_number(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) -> u128 {
    let mut number: u128 = 0;

    while let Some(digit) = characters.peek().and_then(|character| character.to_digit(10)) {
        characters.next();
        number = number.saturating_mul(10).saturating_add(u128::from(digit));
    }

    number
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_says_who_may_do_what() {
        // rw-r-----
        let permissions = Permissions(0o640);

        assert!(permissions.allows(Who::Owner, Access::Read));
        assert!(permissions.allows(Who::Owner, Access::Write));
        assert!(!permissions.allows(Who::Owner, Access::Execute));

        assert!(permissions.allows(Who::Group, Access::Read));
        assert!(!permissions.allows(Who::Group, Access::Write));

        assert!(!permissions.allows(Who::Everyone, Access::Read));

        // And agrees with the shorthand the list column shows.
        assert_eq!(permissions.to_symbolic(), "rw-r-----");
    }

    #[test]
    fn permissions_render_symbolically() {
        assert_eq!(Permissions(0o755).to_symbolic(), "rwxr-xr-x");
        assert_eq!(Permissions(0o640).to_symbolic(), "rw-r-----");
        assert_eq!(Permissions(0o100644).to_symbolic(), "rw-r--r--");
    }

    #[test]
    fn sorting_leads_with_folders_and_reads_numbers_as_numbers() {
        let mut entries = vec![
            Entry::file("file10", 1),
            Entry::file("File2", 1),
            Entry::folder("zebra"),
            Entry::file("file1", 1),
            Entry::folder("apples"),
        ];

        Sort::by_name().apply(&mut entries);

        let names = entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, vec!["apples", "zebra", "file1", "File2", "file10"]);
    }

    #[test]
    fn sorting_by_kind_groups_files_of_a_type_together() {
        let mut entries = vec![
            Entry::file("notes.txt", 1),
            Entry::file("harbour.jpg", 1),
            Entry::file("README", 1),
            Entry::file("diary.txt", 1),
            Entry::file("beach.jpg", 1),
        ];

        Sort { column: Column::Kind, descending: false }.apply(&mut entries);

        let names = entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, vec!["README", "beach.jpg", "harbour.jpg", "diary.txt", "notes.txt"]);
    }

    #[test]
    fn sorting_descending_keeps_folders_on_top() {
        let mut entries = vec![
            Entry::file("a", 30),
            Entry::folder("b"),
            Entry::file("c", 10),
            Entry::folder("a"),
        ];

        Sort { column: Column::Size, descending: true }.apply(&mut entries);

        let names = entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, vec!["b", "a", "a", "c"]);
    }
}
