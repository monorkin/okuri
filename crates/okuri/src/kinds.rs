//! What a file is, worked out from its name.
//!
//! Both the Kind column and the icon are answers to the same question, so they are answered
//! from one table. Two tables drifted apart the first time either was extended: `.css` was code
//! to the icons and an unknown file to the column, and `.conf` was the other way round.

/// A file's sort, as the list names it and as the icon theme names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kind {
    /// What the Kind column says.
    pub label: &'static str,
    /// The icon names to try, most specific first.
    ///
    /// Themes disagree about which of these they ship — one has `application-zip`, another only
    /// `application-x-archive` — so each sort offers its alternatives and the first that exists
    /// wins.
    pub icons: &'static [&'static str],
}

pub const FOLDER: Kind = Kind { label: "Folder", icons: &["folder"] };

const IMAGE: Kind = Kind { label: "Image", icons: &["image-x-generic", "image"] };
const VIDEO: Kind = Kind { label: "Video", icons: &["video-x-generic", "video"] };
const AUDIO: Kind = Kind { label: "Audio", icons: &["audio-x-generic", "audio"] };
const PDF: Kind = Kind { label: "PDF document", icons: &["application-pdf", "x-office-document"] };
const ARCHIVE: Kind = Kind {
    label: "Archive",
    icons: &["package-x-generic", "application-x-archive", "application-zip"],
};
const DOCUMENT: Kind = Kind { label: "Document", icons: &["x-office-document", "text-x-generic"] };
const SPREADSHEET: Kind =
    Kind { label: "Spreadsheet", icons: &["x-office-spreadsheet", "text-x-generic"] };
const PRESENTATION: Kind =
    Kind { label: "Presentation", icons: &["x-office-presentation", "text-x-generic"] };
const CODE: Kind = Kind { label: "Code", icons: &["text-x-script", "text-x-generic"] };
const CONFIGURATION: Kind =
    Kind { label: "Configuration", icons: &["text-x-script", "text-x-generic"] };
const TEXT: Kind = Kind { label: "Text", icons: &["text-x-generic", "text-plain"] };
const UNKNOWN: Kind = Kind { label: "File", icons: &[] };

/// The sort of thing `name` is.
pub fn of(name: &str, is_folder: bool) -> Kind {
    if is_folder {
        return FOLDER;
    }

    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();

    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "svg" | "ico" | "avif" => IMAGE,
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v" | "mpg" | "mpeg" => VIDEO,
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "opus" | "aac" => AUDIO,
        "pdf" => PDF,
        "zip" | "gz" | "bz2" | "xz" | "tar" | "7z" | "rar" | "zst" => ARCHIVE,
        "doc" | "docx" | "odt" | "rtf" => DOCUMENT,
        "xls" | "xlsx" | "ods" | "csv" => SPREADSHEET,
        "ppt" | "pptx" | "odp" => PRESENTATION,
        "rs" | "py" | "js" | "ts" | "rb" | "go" | "c" | "h" | "cpp" | "java" | "sh" | "lua"
        | "html" | "css" | "sql" => CODE,
        "toml" | "json" | "yaml" | "yml" | "xml" | "ini" | "conf" | "cfg" => CONFIGURATION,
        "md" | "txt" | "log" => TEXT,
        _ => UNKNOWN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_known_by_its_extension() {
        assert_eq!(of("harbour.JPG", false).label, "Image");
        assert_eq!(of("notes.md", false).label, "Text");
        assert_eq!(of("okuri.toml", false).label, "Configuration");
        assert_eq!(of("index.html", false).label, "Code");
        assert_eq!(of("photos", true).label, "Folder");
    }

    #[test]
    fn a_file_with_nothing_to_go_on_is_still_a_file() {
        assert_eq!(of("LICENSE", false).label, "File");
        assert_eq!(of("archive.unheardof", false).label, "File");
    }

    /// A dotfile's name is not an extension. `.bashrc` is a file called `.bashrc`, not a file of
    /// type `bashrc` — and certainly not one whose whole name is its suffix.
    #[test]
    fn a_leading_dot_is_a_name_and_not_an_extension() {
        assert_eq!(of(".bashrc", false).label, "File");
    }
}
