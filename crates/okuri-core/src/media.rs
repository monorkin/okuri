//! What kind of thing a file is, in the words HTTP uses.
//!
//! Guessed from the name, because that is all a destination is told at upload time and all a
//! browser has to go on afterwards. Uploading without saying leaves the store to answer
//! `application/octet-stream` to everybody who asks, which is the difference between a browser
//! showing an image and downloading it — and it is invisible until somebody follows a link.

/// The media type for a file called `name`, if it is one we recognise.
///
/// Deliberately a short list of what people actually put in buckets rather than the whole IANA
/// registry: a wrong answer here is worse than none, since none lets the store apply its own
/// default and a wrong one overrides it.
pub fn media_type(name: &str) -> Option<&'static str> {
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();

    let media = match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "ico" => "image/vnd.microsoft.icon",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",

        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",

        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "aac" => "audio/aac",

        "pdf" => "application/pdf",
        "epub" => "application/epub+zip",
        "doc" => "application/msword",
        "docx" => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        }
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "rtf" => "application/rtf",

        "zip" => "application/zip",
        "gz" => "application/gzip",
        "bz2" => "application/x-bzip2",
        "xz" => "application/x-xz",
        "zst" => "application/zstd",
        "tar" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",

        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "md" => "text/markdown",
        "txt" | "log" => "text/plain",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "wasm" => "application/wasm",

        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",

        _ => return None,
    };

    Some(media)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_known_by_its_extension() {
        assert_eq!(media_type("harbour.jpg"), Some("image/jpeg"));
        assert_eq!(media_type("HARBOUR.JPG"), Some("image/jpeg"));
        assert_eq!(media_type("notes.md"), Some("text/markdown"));
        assert_eq!(media_type("site.tar.gz"), Some("application/gzip"));
    }

    /// Better to say nothing than to say the wrong thing: nothing lets the store apply its own
    /// default, and a wrong answer overrides it.
    #[test]
    fn anything_unrecognised_is_left_for_the_store_to_decide() {
        assert_eq!(media_type("LICENSE"), None);
        assert_eq!(media_type("mystery.unheardof"), None);
        assert_eq!(media_type(".bashrc"), None);
    }
}
