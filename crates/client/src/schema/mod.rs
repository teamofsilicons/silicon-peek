//! `peek send` payloads: `--show`, `--ask` and the send options, with the
//! exact limits of BLUEPRINT §7.4.
//!
//! Character counts are Unicode scalar values (`str::chars`). The same types
//! travel on three hops, and only the image reference changes:
//!
//! | hop | image reference |
//! |---|---|
//! | Silicon → CLI (`--show '{…}'`) | a path string, resolved against the CLI's cwd |
//! | CLI → peekd (`send` op) | `{"blob":k}`: the k-th binary blob of the frame |
//! | peekd → Peek.app (`peek.show`) | an absolute cache path string |

pub mod ask;
pub mod send;
pub mod show;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{Error, ErrorCode, Result};

/// Every size and length limit, in one place.
pub mod limits {
    /// `--speak` characters.
    pub const SPEAK_MAX_CHARS: usize = 2000;
    /// Elements in a `--show`.
    pub const SHOW_MAX_ELEMENTS: usize = 3;
    /// Characters in a show text element.
    pub const TEXT_MAX_CHARS: usize = 160;
    /// Characters in an image caption.
    pub const CAPTION_MAX_CHARS: usize = 50;
    /// Characters in an ask question.
    pub const QUESTION_MAX_CHARS: usize = 80;
    /// Characters in a text ask's placeholder.
    pub const PLACEHOLDER_MAX_CHARS: usize = 60;
    /// Largest allowed `max_length` of a text ask.
    pub const TEXT_ANSWER_MAX_LENGTH: u32 = 2000;
    /// Default `max_length` of a text ask.
    pub const TEXT_ANSWER_DEFAULT_MAX_LENGTH: u32 = 500;
    /// Fewest options in a choice ask.
    pub const OPTIONS_MIN: usize = 2;
    /// Most options in a choice ask.
    pub const OPTIONS_MAX: usize = 6;
    /// Characters in an option label.
    pub const OPTION_LABEL_MAX_CHARS: usize = 40;
    /// Characters in an option id (`^[a-z0-9_-]{1,32}$`).
    pub const OPTION_ID_MAX_CHARS: usize = 32;
    /// Characters in a slider or range unit.
    pub const UNIT_MAX_CHARS: usize = 8;
    /// Bytes in one image (show element or option image).
    pub const IMAGE_MAX_BYTES: usize = 10 * 1024 * 1024;
    /// Bytes in a drawing script.
    pub const DRAWING_MAX_BYTES: usize = 256 * 1024;
    /// Bytes in a voice recording (120 s of 16 kHz mono 16-bit, plus header).
    pub const WAV_MAX_BYTES: usize = 4 * 1024 * 1024;
    /// Bytes in one TTS PCM chunk.
    pub const TTS_CHUNK_MAX_BYTES: usize = 64 * 1024;
    /// Seconds in a voice recording.
    pub const RECORDING_MAX_SECONDS: u32 = 120;
    /// `--duration` bounds, seconds.
    pub const DURATION_MIN_S: u64 = 1;
    /// `--duration` upper bound, seconds.
    pub const DURATION_MAX_S: u64 = 120;
    /// `--expires-in` lower bound, seconds.
    pub const EXPIRES_IN_MIN_S: u64 = 10;
    /// `--expires-in` upper bound, seconds (7 days).
    pub const EXPIRES_IN_MAX_S: u64 = 7 * 24 * 3600;
    /// `--wait` lower bound, seconds.
    pub const WAIT_MIN_S: u64 = 1;
    /// `--wait` upper bound, seconds.
    pub const WAIT_MAX_S: u64 = 600;
    /// `--wait` default, seconds.
    pub const WAIT_DEFAULT_S: u64 = 120;
    /// Characters in an ISI.
    pub const ISI_MAX_CHARS: usize = 160;
    /// Sends that may queue behind a pending ask in one slot.
    pub const QUEUE_MAX: usize = 5;
    /// Largest `history --limit`.
    pub const HISTORY_MAX_LIMIT: u32 = 200;
}

/// A reference to image bytes; see the module table for which form each hop
/// uses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ImageRef {
    /// A filesystem path (CLI input: relative to cwd; UI: absolute cache path).
    Path(String),
    /// The index of a binary blob in the same IPC frame.
    Blob {
        /// Zero-based blob index.
        blob: usize,
    },
}

/// Which image reference form a hop accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageHop {
    /// Silicon → CLI: non-empty path strings.
    Input,
    /// CLI → peekd: blob indices below the frame's blob count.
    Ipc {
        /// Number of blobs in the frame.
        blobs: usize,
    },
    /// peekd → Peek.app: absolute path strings.
    Ui,
}

pub(crate) fn check_image_ref(field: &str, image: &ImageRef, hop: ImageHop) -> Result<()> {
    match (hop, image) {
        (ImageHop::Input, ImageRef::Path(p)) if !p.is_empty() && !p.contains('\0') => Ok(()),
        (ImageHop::Ui, ImageRef::Path(p)) if p.starts_with('/') && !p.contains('\0') => Ok(()),
        (ImageHop::Ipc { blobs }, ImageRef::Blob { blob }) if *blob < blobs => Ok(()),
        (ImageHop::Ipc { blobs }, ImageRef::Blob { blob }) => Err(Error::invalid_input(format!(
            "`{field}` references blob {blob}, but the frame carries {blobs} blob(s)"
        ))),
        (ImageHop::Input, _) => Err(Error::invalid_input(format!(
            "`{field}` must be a non-empty image path (relative to the current directory)"
        ))),
        (ImageHop::Ui, _) => Err(Error::invalid_input(format!(
            "`{field}` must be an absolute image cache path"
        ))),
        (ImageHop::Ipc { .. }, ImageRef::Path(_)) => Err(Error::invalid_input(format!(
            "`{field}` must reference a frame blob as {{\"blob\":k}}; the CLI sends image bytes, never paths"
        ))),
    }
}

/// Whether control characters are acceptable in a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lines {
    /// One line: no control characters at all.
    Single,
    /// Speech: newlines and tabs are allowed, other controls are not.
    Multi,
}

/// Validates a text field's character count and characters.
pub(crate) fn check_text(
    field: &str,
    value: &str,
    min: usize,
    max: usize,
    too_long: ErrorCode,
    lines: Lines,
) -> Result<()> {
    let count = value.chars().count();
    if count > max {
        return Err(Error::new(
            too_long,
            format!("`{field}` has {count} characters; the limit is {max}"),
        )
        .with_hint(format!("shorten `{field}` to at most {max} characters"))
        .with_details(json!({"field": field, "limit": max, "actual": count})));
    }
    if count < min {
        return Err(Error::invalid_input(format!(
            "`{field}` is empty; it needs at least {min} character(s)"
        )));
    }
    if min > 0 && value.trim().is_empty() {
        return Err(Error::invalid_input(format!(
            "`{field}` contains only whitespace; give it visible text"
        )));
    }
    let bad = value
        .chars()
        .find(|c| c.is_control() && !(lines == Lines::Multi && matches!(c, '\n' | '\t')));
    if let Some(c) = bad {
        return Err(Error::invalid_input(format!(
            "`{field}` contains the control character U+{:04X}; {}",
            u32::from(c),
            if lines == Lines::Single {
                "it is shown on one line, so remove line breaks and control characters"
            } else {
                "only line breaks and tabs are allowed"
            }
        )));
    }
    Ok(())
}

/// Image formats Peek.app can render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    /// PNG.
    Png,
    /// JPEG.
    Jpeg,
    /// HEIC/HEIF.
    Heic,
    /// WebP.
    Webp,
    /// GIF (the first frame is shown).
    Gif,
}

impl ImageFormat {
    /// Detects the format from the file's magic bytes (never the extension).
    #[must_use]
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else if bytes.len() >= 12
            && &bytes[4..8] == b"ftyp"
            && matches!(
                &bytes[8..12],
                b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1"
            )
        {
            Some(Self::Heic)
        } else {
            None
        }
    }

    /// The conventional file extension.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Heic => "heic",
            Self::Webp => "webp",
            Self::Gif => "gif",
        }
    }

    /// The media type.
    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Heic => "image/heic",
            Self::Webp => "image/webp",
            Self::Gif => "image/gif",
        }
    }
}

/// Checks image bytes read from `path` (named in messages): at most 10 MiB and
/// a supported format.
///
/// # Errors
/// `image_too_large` or `image_unsupported`.
pub fn check_image_bytes(path: &str, bytes: &[u8]) -> Result<ImageFormat> {
    if bytes.len() > limits::IMAGE_MAX_BYTES {
        return Err(Error::new(
            ErrorCode::ImageTooLarge,
            format!(
                "image `{path}` is {} bytes; the limit is {} bytes (10 MiB)",
                bytes.len(),
                limits::IMAGE_MAX_BYTES
            ),
        )
        .with_hint("downscale it; Peek shows images at most 512 px")
        .with_details(
            json!({"path": path, "limit": limits::IMAGE_MAX_BYTES, "actual": bytes.len()}),
        ));
    }
    ImageFormat::sniff(bytes).ok_or_else(|| {
        Error::new(
            ErrorCode::ImageUnsupported,
            format!("image `{path}` is not PNG, JPEG, HEIC, WebP or GIF (checked by content, not extension)"),
        )
        .with_hint("convert it, for example: sips -s format png in.tiff --out out.png")
    })
}

/// Checks a drawing script's size.
///
/// # Errors
/// `drawing_too_large` above 256 KiB; `invalid_input` for an empty file.
pub fn check_drawing_bytes(filename: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() > limits::DRAWING_MAX_BYTES {
        return Err(Error::new(
            ErrorCode::DrawingTooLarge,
            format!(
                "drawing `{filename}` is {} bytes; the limit is {} bytes (256 KiB)",
                bytes.len(),
                limits::DRAWING_MAX_BYTES
            ),
        )
        .with_hint("minify the script or move large data out of it")
        .with_details(json!({"limit": limits::DRAWING_MAX_BYTES, "actual": bytes.len()})));
    }
    if bytes.is_empty() {
        return Err(Error::invalid_input(format!(
            "drawing `{filename}` is empty"
        )));
    }
    if std::str::from_utf8(bytes).is_err() {
        return Err(Error::invalid_input(format!(
            "drawing `{filename}` is not UTF-8 JavaScript"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffing() {
        assert_eq!(
            ImageFormat::sniff(b"\x89PNG\r\n\x1a\nrest"),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            ImageFormat::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(ImageFormat::sniff(b"GIF89a...."), Some(ImageFormat::Gif));
        assert_eq!(
            ImageFormat::sniff(b"RIFF\0\0\0\0WEBPVP8 "),
            Some(ImageFormat::Webp)
        );
        assert_eq!(
            ImageFormat::sniff(b"\0\0\0\x18ftypheic\0\0"),
            Some(ImageFormat::Heic)
        );
        assert_eq!(ImageFormat::sniff(b"\0\0\0\x18ftypavif\0\0"), None);
        assert_eq!(ImageFormat::sniff(b"BM"), None);
        assert_eq!(ImageFormat::sniff(b""), None);
    }

    #[test]
    fn image_and_drawing_size_boundaries() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.resize(limits::IMAGE_MAX_BYTES, 0);
        assert!(check_image_bytes("a.png", &png).is_ok());
        png.push(0);
        let e = check_image_bytes("a.png", &png).err();
        assert!(e.is_some_and(|e| *e.code() == ErrorCode::ImageTooLarge));
        let e = check_image_bytes("a.tiff", b"II*\0").err();
        assert!(e.is_some_and(|e| *e.code() == ErrorCode::ImageUnsupported));

        let js = vec![b' '; limits::DRAWING_MAX_BYTES];
        assert!(check_drawing_bytes("a.js", &js).is_ok());
        let js = vec![b' '; limits::DRAWING_MAX_BYTES + 1];
        let e = check_drawing_bytes("a.js", &js).err();
        assert!(e.is_some_and(|e| *e.code() == ErrorCode::DrawingTooLarge));
        assert!(check_drawing_bytes("a.js", b"").is_err());
        assert!(check_drawing_bytes("a.js", b"\xff\xfe").is_err());
    }

    #[test]
    fn text_checks() {
        let ok = check_text("f", "héllo", 1, 5, ErrorCode::TextTooLong, Lines::Single);
        assert!(ok.is_ok(), "5 scalar values fit a 5-char limit");
        let e = check_text("f", "héllo!", 1, 5, ErrorCode::TextTooLong, Lines::Single).err();
        assert!(e.is_some_and(|e| *e.code() == ErrorCode::TextTooLong
            && e.details().is_some_and(|d| d["actual"] == 6)));
        assert!(check_text("f", "", 1, 5, ErrorCode::TextTooLong, Lines::Single).is_err());
        assert!(check_text("f", "   ", 1, 5, ErrorCode::TextTooLong, Lines::Single).is_err());
        assert!(check_text("f", "", 0, 5, ErrorCode::TextTooLong, Lines::Single).is_ok());
        assert!(check_text("f", "a\nb", 1, 5, ErrorCode::TextTooLong, Lines::Single).is_err());
        assert!(check_text("f", "a\nb", 1, 5, ErrorCode::TextTooLong, Lines::Multi).is_ok());
        assert!(check_text("f", "a\u{7}b", 1, 5, ErrorCode::TextTooLong, Lines::Multi).is_err());
        // Emoji are single scalar values; combining marks count separately.
        assert!(check_text("f", "👍👍", 1, 2, ErrorCode::TextTooLong, Lines::Single).is_ok());
        assert!(
            check_text(
                "f",
                "e\u{301}e\u{301}",
                1,
                3,
                ErrorCode::TextTooLong,
                Lines::Single
            )
            .is_err()
        );
    }

    #[test]
    fn image_refs_per_hop() {
        let p = ImageRef::Path("./a.png".into());
        let abs = ImageRef::Path("/cache/a.png".into());
        let b = ImageRef::Blob { blob: 1 };
        assert!(check_image_ref("x", &p, ImageHop::Input).is_ok());
        assert!(check_image_ref("x", &b, ImageHop::Input).is_err());
        assert!(check_image_ref("x", &b, ImageHop::Ipc { blobs: 2 }).is_ok());
        assert!(check_image_ref("x", &b, ImageHop::Ipc { blobs: 1 }).is_err());
        assert!(check_image_ref("x", &p, ImageHop::Ipc { blobs: 2 }).is_err());
        assert!(check_image_ref("x", &abs, ImageHop::Ui).is_ok());
        assert!(check_image_ref("x", &p, ImageHop::Ui).is_err());
    }
}
