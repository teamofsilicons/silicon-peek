//! `--show`: one to three text or image elements laid out on the information
//! arc (BLUEPRINT §7.4).
//!
//! ```json
//! {"elements":[{"type":"text","text":"Now playing"},
//!              {"type":"image","path":"./covers/co2.jpg","caption":"CO2 by Prateek Kuhad"}]}
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{ImageHop, ImageRef, Lines, check_image_ref, check_text, limits};
use crate::{
    error::{Error, ErrorCode, Result},
    json::kind,
};

/// A validated show.
///
/// Deserialization is lenient (IPC ignores unknown fields); call
/// [`Show::validate`] on anything received. Silicon input goes through
/// [`Show::from_input`], which is strict.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Show {
    /// One to three elements, in display order.
    pub elements: Vec<ShowElement>,
}

/// One show element.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShowElement {
    /// A text pill (1–160 characters, one line).
    Text {
        /// The text.
        text: String,
    },
    /// An image with an optional caption (0–50 characters).
    Image {
        /// The image reference for this hop.
        path: ImageRef,
        /// The caption, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
    },
}

impl Show {
    /// Parses a Silicon's `--show` JSON strictly: unknown fields, wrong types
    /// and every limit are refused with the exact field named.
    ///
    /// # Errors
    /// `invalid_input`, `too_many_elements`, `text_too_long`, `caption_too_long`.
    pub fn from_input(value: &Value) -> Result<Self> {
        Self::parse_input(value).map_err(|e| {
            e.with_input_context(
                "show",
                "see `peek docs show` for the element types, their fields and limits",
            )
        })
    }

    fn parse_input(value: &Value) -> Result<Self> {
        let obj = value.as_object().ok_or_else(|| {
            Error::invalid_input(format!("--show must be a JSON object, got {}", kind(value)))
        })?;
        reject_unknown(obj, "--show", &["elements"])?;
        let elements = obj
            .get("elements")
            .ok_or_else(|| {
                Error::invalid_input("--show needs `elements`: an array of 1–3 elements")
                    .with_hint(r#"--show '{"elements":[{"type":"text","text":"Hello"}]}'"#)
            })?
            .as_array()
            .ok_or_else(|| Error::invalid_input("`show.elements` must be an array"))?;
        check_element_count(elements.len())?;
        let mut out = Vec::with_capacity(elements.len());
        for (i, e) in elements.iter().enumerate() {
            let field = format!("show.elements[{i}]");
            let e = e.as_object().ok_or_else(|| {
                Error::invalid_input(format!("`{field}` must be an object, got {}", kind(e)))
            })?;
            let ty = e.get("type").and_then(Value::as_str).ok_or_else(|| {
                Error::invalid_input(format!("`{field}.type` is required: \"text\" or \"image\""))
            })?;
            match ty {
                "text" => {
                    reject_unknown(e, &field, &["type", "text"])?;
                    let text = string_field(e, &field, "text")?.ok_or_else(|| {
                        Error::invalid_input(format!("`{field}.text` is required"))
                    })?;
                    out.push(ShowElement::Text { text });
                }
                "image" => {
                    reject_unknown(e, &field, &["type", "path", "caption"])?;
                    let path = string_field(e, &field, "path")?.ok_or_else(|| {
                        Error::invalid_input(format!(
                            "`{field}.path` is required: the image file, relative to the current directory"
                        ))
                    })?;
                    let caption = string_field(e, &field, "caption")?;
                    out.push(ShowElement::Image {
                        path: ImageRef::Path(path),
                        caption,
                    });
                }
                other => {
                    return Err(Error::invalid_input(format!(
                        "`{field}.type` is `{other}`; expected \"text\" or \"image\""
                    )));
                }
            }
        }
        let show = Self { elements: out };
        show.validate(ImageHop::Input)?;
        Ok(show)
    }

    /// Checks every limit, and that image references suit `hop`.
    ///
    /// # Errors
    /// As [`Show::from_input`].
    pub fn validate(&self, hop: ImageHop) -> Result<()> {
        check_element_count(self.elements.len())?;
        for (i, e) in self.elements.iter().enumerate() {
            match e {
                ShowElement::Text { text } => check_text(
                    &format!("show.elements[{i}].text"),
                    text,
                    1,
                    limits::TEXT_MAX_CHARS,
                    ErrorCode::TextTooLong,
                    Lines::Single,
                )?,
                ShowElement::Image { path, caption } => {
                    check_image_ref(&format!("show.elements[{i}].path"), path, hop)?;
                    if let Some(c) = caption {
                        check_text(
                            &format!("show.elements[{i}].caption"),
                            c,
                            0,
                            limits::CAPTION_MAX_CHARS,
                            ErrorCode::CaptionTooLong,
                            Lines::Single,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Every image reference, in element order, for rewriting paths to blobs
    /// (CLI) or blobs to cache paths (peekd).
    pub fn images_mut(&mut self) -> impl Iterator<Item = &mut ImageRef> {
        self.elements.iter_mut().filter_map(|e| match e {
            ShowElement::Image { path, .. } => Some(path),
            ShowElement::Text { .. } => None,
        })
    }

    /// Every image reference, in element order.
    pub fn images(&self) -> impl Iterator<Item = &ImageRef> {
        self.elements.iter().filter_map(|e| match e {
            ShowElement::Image { path, .. } => Some(path),
            ShowElement::Text { .. } => None,
        })
    }

    /// Visible characters: text plus captions.
    #[must_use]
    pub fn visible_chars(&self) -> usize {
        self.elements
            .iter()
            .map(|e| match e {
                ShowElement::Text { text } => text.chars().count(),
                ShowElement::Image { caption, .. } => {
                    caption.as_deref().map_or(0, |c| c.chars().count())
                }
            })
            .sum()
    }

    /// How long the show stays up without `--speak` (or after speech ends)
    /// when `--duration` is not given: `clamp(3 + 0.06 × visible chars, 4, 15)` s.
    #[must_use]
    pub fn default_duration(&self) -> Duration {
        // Integer milliseconds: 3 s + 60 ms per character, exactly.
        let chars = u64::try_from(self.visible_chars()).unwrap_or(u64::MAX);
        let ms = chars
            .saturating_mul(60)
            .saturating_add(3000)
            .clamp(4000, 15_000);
        Duration::from_millis(ms)
    }

    /// Element kinds, for telemetry (`show_kinds`); never the content.
    #[must_use]
    pub fn kinds(&self) -> Vec<&'static str> {
        self.elements
            .iter()
            .map(|e| match e {
                ShowElement::Text { .. } => "text",
                ShowElement::Image { .. } => "image",
            })
            .collect()
    }
}

fn check_element_count(n: usize) -> Result<()> {
    if n > limits::SHOW_MAX_ELEMENTS {
        return Err(Error::new(
            ErrorCode::TooManyElements,
            format!(
                "--show has {n} elements; the limit is {} (each takes up to a third of the arc)",
                limits::SHOW_MAX_ELEMENTS
            ),
        )
        .with_hint("split the content over several sends, or drop an element")
        .with_details(json!({"limit": limits::SHOW_MAX_ELEMENTS, "actual": n})));
    }
    if n == 0 {
        return Err(Error::invalid_input(
            "`show.elements` is empty; give 1–3 text or image elements",
        ));
    }
    Ok(())
}

pub(crate) fn reject_unknown(
    obj: &Map<String, Value>,
    field: &str,
    allowed: &[&str],
) -> Result<()> {
    if let Some(k) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(Error::invalid_input(format!(
            "unknown field `{k}` in `{field}`; allowed: {}",
            allowed.join(", ")
        ))
        .with_details(json!({"field": field, "unknown": k, "allowed": allowed})));
    }
    Ok(())
}

pub(crate) fn string_field(
    obj: &Map<String, Value>,
    field: &str,
    key: &str,
) -> Result<Option<String>> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(Error::invalid_input(format!(
            "`{field}.{key}` must be a string, got {}",
            kind(other)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::needless_pass_by_value)] // call sites pass json! literals
    fn parse(v: Value) -> Result<Show> {
        Show::from_input(&v)
    }

    fn code(r: Result<Show>) -> Option<ErrorCode> {
        r.err().map(|e| e.code().clone())
    }

    #[test]
    fn accepts_the_spec_example() -> Result<()> {
        let s = parse(json!({"elements":[
            {"type":"text","text":"Now playing"},
            {"type":"image","path":"./covers/co2.jpg","caption":"CO2 by Prateek Kuhad"}
        ]}))?;
        assert_eq!(s.elements.len(), 2);
        assert_eq!(s.visible_chars(), 11 + 20);
        assert_eq!(s.kinds(), ["text", "image"]);
        Ok(())
    }

    #[test]
    fn element_count_boundaries() {
        let t = json!({"type":"text","text":"x"});
        assert_eq!(
            code(parse(json!({"elements":[]}))),
            Some(ErrorCode::InvalidInput)
        );
        assert!(parse(json!({"elements":[t.clone()]})).is_ok());
        assert!(parse(json!({"elements":[t.clone(), t.clone(), t.clone()]})).is_ok());
        assert_eq!(
            code(parse(
                json!({"elements":[t.clone(), t.clone(), t.clone(), t]})
            )),
            Some(ErrorCode::TooManyElements)
        );
    }

    #[test]
    fn text_and_caption_boundaries() {
        let text = |n: usize| json!({"elements":[{"type":"text","text":"é".repeat(n)}]});
        assert!(parse(text(160)).is_ok());
        assert_eq!(code(parse(text(161))), Some(ErrorCode::TextTooLong));
        assert_eq!(code(parse(text(0))), Some(ErrorCode::InvalidInput));
        let cap = |n: usize| json!({"elements":[{"type":"image","path":"a.png","caption":"c".repeat(n)}]});
        assert!(parse(cap(0)).is_ok());
        assert!(parse(cap(50)).is_ok());
        assert_eq!(code(parse(cap(51))), Some(ErrorCode::CaptionTooLong));
    }

    #[test]
    fn strictness() {
        assert!(parse(json!([])).is_err());
        assert!(parse(json!({"elements":[], "extra":1})).is_err());
        assert!(parse(json!({"elements":[{"type":"text","text":"x","color":"red"}]})).is_err());
        assert!(parse(json!({"elements":[{"type":"video","path":"a.mp4"}]})).is_err());
        assert!(parse(json!({"elements":[{"type":"image"}]})).is_err());
        assert!(parse(json!({"elements":[{"type":"image","path":""}]})).is_err());
        assert!(parse(json!({"elements":[{"type":"text","text":7}]})).is_err());
        assert!(parse(json!({"elements":[{"text":"x"}]})).is_err());
        assert!(parse(json!({"elements":[{"type":"text","text":"two\nlines"}]})).is_err());
    }

    #[test]
    fn default_duration_is_clamped() -> Result<()> {
        let s = parse(json!({"elements":[{"type":"text","text":"hi"}]}))?;
        assert_eq!(s.default_duration(), Duration::from_secs(4));
        let s = parse(
            json!({"elements":[{"type":"text","text":"x".repeat(160)},{"type":"text","text":"y".repeat(160)}]}),
        )?;
        assert_eq!(s.default_duration(), Duration::from_secs(15));
        let s = parse(json!({"elements":[{"type":"text","text":"x".repeat(50)}]}))?;
        assert_eq!(s.default_duration(), Duration::from_secs(6));
        Ok(())
    }

    #[test]
    fn ipc_round_trip_with_blobs() -> Result<()> {
        let mut s = parse(json!({"elements":[{"type":"image","path":"a.png","caption":"c"}]}))?;
        for (k, img) in s.images_mut().enumerate() {
            *img = ImageRef::Blob { blob: k };
        }
        s.validate(ImageHop::Ipc { blobs: 1 })?;
        let wire = serde_json::to_value(&s).map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(
            wire,
            json!({"elements":[{"type":"image","path":{"blob":0},"caption":"c"}]})
        );
        let back: Show =
            serde_json::from_value(wire).map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(back, s);
        assert!(back.validate(ImageHop::Ipc { blobs: 0 }).is_err());
        Ok(())
    }
}
