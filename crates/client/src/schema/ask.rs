//! `--ask`: one self-contained question (BLUEPRINT §7.4), and the answer
//! shapes that travel back to the Silicon in `peek.ask.answered`.
//!
//! ```json
//! {"question":"Delete ~/Downloads/old.zip?","type":"single_choice",
//!  "options":[{"id":"keep","label":"Keep"},{"id":"delete","label":"Delete","image":"./trash.png"}]}
//! ```

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{
    ImageHop, ImageRef, Lines, check_image_ref, check_text, limits,
    show::{reject_unknown, string_field},
};
use crate::{
    error::{Error, ErrorCode, Result},
    json::kind,
    num::Num,
};

/// The five ask types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskType {
    /// Free text.
    Text,
    /// Exactly one option.
    SingleChoice,
    /// Between `min` and `max` options.
    MultipleChoice,
    /// One number on a scale.
    Slider,
    /// Two numbers on a scale.
    Range,
}

impl AskType {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::SingleChoice => "single_choice",
            Self::MultipleChoice => "multiple_choice",
            Self::Slider => "slider",
            Self::Range => "range",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "text" => Self::Text,
            "single_choice" => Self::SingleChoice,
            "multiple_choice" => Self::MultipleChoice,
            "slider" => Self::Slider,
            "range" => Self::Range,
            _ => return None,
        })
    }

    fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Text => &["question", "type", "placeholder", "max_length"],
            Self::SingleChoice => &["question", "type", "options"],
            Self::MultipleChoice => &["question", "type", "options", "min", "max"],
            Self::Slider | Self::Range => {
                &["question", "type", "min", "max", "step", "default", "unit"]
            }
        }
    }
}

/// A validated ask. Serializes as `{"question", "type", …type fields}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    /// The question, 1–80 characters, always text.
    pub question: String,
    /// The type and its fields.
    #[serde(flatten)]
    pub kind: AskKind,
}

/// Type-specific ask fields, with every default filled in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AskKind {
    /// Free text.
    Text {
        /// Placeholder in the text field (0–60 characters).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        /// Longest accepted answer (1–2000, default 500).
        max_length: u32,
    },
    /// Exactly one of 2–6 options.
    SingleChoice {
        /// The options.
        options: Vec<AskOption>,
    },
    /// Between `min` and `max` of 2–6 options.
    MultipleChoice {
        /// The options.
        options: Vec<AskOption>,
        /// Fewest selections (default 1).
        min: u32,
        /// Most selections (default: every option).
        max: u32,
    },
    /// One number in `[min, max]`.
    Slider {
        /// Lower bound.
        min: Num,
        /// Upper bound (greater than `min`).
        max: Num,
        /// Increment (default `(max - min) / 100`).
        step: Num,
        /// Initial value (default `min`).
        default: Num,
        /// Unit label, 0–8 characters.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    /// Two numbers `from ≤ to` in `[min, max]`.
    Range {
        /// Lower bound.
        min: Num,
        /// Upper bound (greater than `min`).
        max: Num,
        /// Increment (default `(max - min) / 100`).
        step: Num,
        /// Initial `[from, to]` (default `[min, max]`).
        default: [Num; 2],
        /// Unit label, 0–8 characters.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
}

/// One option of a choice ask.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskOption {
    /// `^[a-z0-9_-]{1,32}$`, unique; defaults to `"1"`, `"2"`, … by position.
    pub id: String,
    /// 1–40 characters.
    pub label: String,
    /// An optional image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageRef>,
}

/// The answer, exactly as delivered in `peek.ask.answered.data.answer`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Answer {
    /// A text answer.
    Text {
        /// The text.
        text: String,
    },
    /// One option.
    SingleChoice {
        /// The chosen option's id.
        option_id: String,
        /// Its label.
        label: String,
    },
    /// Several options, in the ask's option order.
    MultipleChoice {
        /// The chosen ids.
        option_ids: Vec<String>,
        /// Their labels, index-aligned with `option_ids`.
        labels: Vec<String>,
    },
    /// A slider value.
    Slider {
        /// The value.
        value: Num,
    },
    /// A range.
    Range {
        /// Lower end.
        from: Num,
        /// Upper end.
        to: Num,
    },
}

impl Answer {
    /// The ask type this answer belongs to.
    #[must_use]
    pub const fn ask_type(&self) -> AskType {
        match self {
            Self::Text { .. } => AskType::Text,
            Self::SingleChoice { .. } => AskType::SingleChoice,
            Self::MultipleChoice { .. } => AskType::MultipleChoice,
            Self::Slider { .. } => AskType::Slider,
            Self::Range { .. } => AskType::Range,
        }
    }
}

impl Ask {
    /// Parses a Silicon's `--ask` JSON strictly, filling every default.
    ///
    /// # Errors
    /// `invalid_input`, `question_too_long`, `too_many_options`, `text_too_long`.
    pub fn from_input(value: &Value) -> Result<Self> {
        Self::parse_input(value).map_err(|e| {
            e.with_input_context(
                "ask",
                "see `peek docs ask` for every ask type, its fields and limits",
            )
        })
    }

    fn parse_input(value: &Value) -> Result<Self> {
        let obj = value.as_object().ok_or_else(|| {
            Error::invalid_input(format!("--ask must be a JSON object, got {}", kind(value)))
        })?;
        let ty_value = obj.get("type").ok_or_else(|| {
            Error::invalid_input(
                "--ask needs `type`: text, single_choice, multiple_choice, slider or range",
            )
        })?;
        let ty = ty_value.as_str().and_then(AskType::parse).ok_or_else(|| {
            Error::invalid_input(format!(
                "`ask.type` is {}; expected text, single_choice, multiple_choice, slider or range",
                describe(ty_value)
            ))
        })?;
        for k in obj.keys() {
            if !ty.fields().contains(&k.as_str()) {
                let known = [
                    AskType::Text,
                    AskType::SingleChoice,
                    AskType::MultipleChoice,
                    AskType::Slider,
                ]
                .iter()
                .any(|t| t.fields().contains(&k.as_str()));
                let msg = if known {
                    format!("field `{k}` does not apply to ask type {}", ty.as_str())
                } else {
                    format!("unknown field `{k}` in --ask")
                };
                return Err(Error::invalid_input(format!(
                    "{msg}; allowed for {}: {}",
                    ty.as_str(),
                    ty.fields().join(", ")
                )));
            }
        }
        let question = string_field(obj, "ask", "question")?.ok_or_else(|| {
            Error::invalid_input("--ask needs `question`: 1–80 characters of text")
        })?;
        let kind = match ty {
            AskType::Text => AskKind::Text {
                placeholder: string_field(obj, "ask", "placeholder")?.filter(|p| !p.is_empty()),
                max_length: match obj.get("max_length") {
                    None | Some(Value::Null) => limits::TEXT_ANSWER_DEFAULT_MAX_LENGTH,
                    Some(v) => u32_field(v, "ask.max_length")?,
                },
            },
            AskType::SingleChoice => AskKind::SingleChoice {
                options: parse_options(obj)?,
            },
            AskType::MultipleChoice => {
                let options = parse_options(obj)?;
                let n = u32::try_from(options.len()).unwrap_or(u32::MAX);
                let min = match obj.get("min") {
                    None | Some(Value::Null) => 1,
                    Some(v) => u32_field(v, "ask.min")?,
                };
                let max = match obj.get("max") {
                    None | Some(Value::Null) => n,
                    Some(v) => u32_field(v, "ask.max")?,
                };
                AskKind::MultipleChoice { options, min, max }
            }
            AskType::Slider | AskType::Range => {
                let min = num_field(obj, "min")?
                    .ok_or_else(|| Error::invalid_input("`ask.min` is required for a scale"))?;
                let max = num_field(obj, "max")?
                    .ok_or_else(|| Error::invalid_input("`ask.max` is required for a scale"))?;
                let step = match num_field(obj, "step")? {
                    Some(s) => s,
                    None => Num::new((max.get() - min.get()) / 100.0).unwrap_or_default(),
                };
                let unit = string_field(obj, "ask", "unit")?.filter(|u| !u.is_empty());
                if ty == AskType::Slider {
                    let default = num_field(obj, "default")?.unwrap_or(min);
                    AskKind::Slider {
                        min,
                        max,
                        step,
                        default,
                        unit,
                    }
                } else {
                    let default = match obj.get("default") {
                        None | Some(Value::Null) => [min, max],
                        Some(v) => pair_field(v, "ask.default")?,
                    };
                    AskKind::Range {
                        min,
                        max,
                        step,
                        default,
                        unit,
                    }
                }
            }
        };
        let ask = Self { question, kind };
        ask.validate(ImageHop::Input)?;
        Ok(ask)
    }

    /// The type.
    #[must_use]
    pub fn ask_type(&self) -> AskType {
        match self.kind {
            AskKind::Text { .. } => AskType::Text,
            AskKind::SingleChoice { .. } => AskType::SingleChoice,
            AskKind::MultipleChoice { .. } => AskType::MultipleChoice,
            AskKind::Slider { .. } => AskType::Slider,
            AskKind::Range { .. } => AskType::Range,
        }
    }

    /// The options of a choice ask (empty otherwise).
    #[must_use]
    pub fn options(&self) -> &[AskOption] {
        match &self.kind {
            AskKind::SingleChoice { options } | AskKind::MultipleChoice { options, .. } => options,
            _ => &[],
        }
    }

    /// Checks every limit and rule, and that image references suit `hop`.
    ///
    /// # Errors
    /// As [`Ask::from_input`].
    pub fn validate(&self, hop: ImageHop) -> Result<()> {
        check_text(
            "ask.question",
            &self.question,
            1,
            limits::QUESTION_MAX_CHARS,
            ErrorCode::QuestionTooLong,
            Lines::Single,
        )?;
        match &self.kind {
            AskKind::Text {
                placeholder,
                max_length,
            } => {
                if let Some(p) = placeholder {
                    check_text(
                        "ask.placeholder",
                        p,
                        0,
                        limits::PLACEHOLDER_MAX_CHARS,
                        ErrorCode::TextTooLong,
                        Lines::Single,
                    )?;
                }
                if !(1..=limits::TEXT_ANSWER_MAX_LENGTH).contains(max_length) {
                    return Err(Error::invalid_input(format!(
                        "`ask.max_length` is {max_length}; it must be 1–{}",
                        limits::TEXT_ANSWER_MAX_LENGTH
                    )));
                }
            }
            AskKind::SingleChoice { options } => check_options(options, hop)?,
            AskKind::MultipleChoice { options, min, max } => {
                check_options(options, hop)?;
                let n = u32::try_from(options.len()).unwrap_or(u32::MAX);
                if *max < 1 || *max > n {
                    return Err(Error::invalid_input(format!(
                        "`ask.max` is {max}; it must be 1–{n} (the number of options)"
                    )));
                }
                if min > max {
                    return Err(Error::invalid_input(format!(
                        "`ask.min` ({min}) is greater than `ask.max` ({max})"
                    )));
                }
            }
            AskKind::Slider {
                min,
                max,
                step,
                default,
                unit,
            } => {
                check_scale(*min, *max, *step, unit.as_deref())?;
                if default < min || default > max {
                    return Err(Error::invalid_input(format!(
                        "`ask.default` ({default}) must lie within [{min}, {max}]"
                    )));
                }
            }
            AskKind::Range {
                min,
                max,
                step,
                default: [from, to],
                unit,
            } => {
                check_scale(*min, *max, *step, unit.as_deref())?;
                if from < min || to > max || from > to {
                    return Err(Error::invalid_input(format!(
                        "`ask.default` [{from}, {to}] must satisfy {min} ≤ from ≤ to ≤ {max}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Every option image reference, in option order.
    pub fn images_mut(&mut self) -> impl Iterator<Item = &mut ImageRef> {
        let options: &mut [AskOption] = match &mut self.kind {
            AskKind::SingleChoice { options } | AskKind::MultipleChoice { options, .. } => options,
            _ => &mut [],
        };
        options.iter_mut().filter_map(|o| o.image.as_mut())
    }

    /// Every option image reference, in option order.
    pub fn images(&self) -> impl Iterator<Item = &ImageRef> {
        self.options().iter().filter_map(|o| o.image.as_ref())
    }

    /// Turns the raw value Peek.app reports (`answer.value`) into the typed
    /// answer, enforcing the ask's own rules. Values by type: text → string;
    /// single choice → option id; multiple choice → array of option ids;
    /// slider → number; range → `[from, to]`.
    ///
    /// # Errors
    /// `invalid_input` when the value does not answer this ask.
    pub fn resolve_answer(&self, value: &Value) -> Result<Answer> {
        match &self.kind {
            AskKind::Text { max_length, .. } => {
                let text = value.as_str().ok_or_else(|| {
                    Error::invalid_input(format!(
                        "a text answer must be a string, got {}",
                        kind(value)
                    ))
                })?;
                let text = text.trim();
                let max = usize::try_from(*max_length).unwrap_or(usize::MAX);
                check_text("answer", text, 1, max, ErrorCode::TextTooLong, Lines::Multi)?;
                Ok(Answer::Text {
                    text: text.to_owned(),
                })
            }
            AskKind::SingleChoice { options } => {
                let id = value.as_str().ok_or_else(|| {
                    Error::invalid_input("a single-choice answer must be an option id string")
                })?;
                let o = find_option(options, id)?;
                Ok(Answer::SingleChoice {
                    option_id: o.id.clone(),
                    label: o.label.clone(),
                })
            }
            AskKind::MultipleChoice { options, min, max } => {
                let ids = value.as_array().ok_or_else(|| {
                    Error::invalid_input("a multiple-choice answer must be an array of option ids")
                })?;
                let mut chosen = Vec::with_capacity(ids.len());
                for id in ids {
                    let id = id.as_str().ok_or_else(|| {
                        Error::invalid_input("multiple-choice option ids must be strings")
                    })?;
                    find_option(options, id)?;
                    if chosen.contains(&id) {
                        return Err(Error::invalid_input(format!(
                            "option `{id}` is selected twice"
                        )));
                    }
                    chosen.push(id);
                }
                let n = u32::try_from(chosen.len()).unwrap_or(u32::MAX);
                if n < *min || n > *max {
                    return Err(Error::invalid_input(format!(
                        "{n} option(s) selected; this ask needs {min}–{max}"
                    )));
                }
                let picked: Vec<&AskOption> = options
                    .iter()
                    .filter(|o| chosen.contains(&o.id.as_str()))
                    .collect();
                Ok(Answer::MultipleChoice {
                    option_ids: picked.iter().map(|o| o.id.clone()).collect(),
                    labels: picked.iter().map(|o| o.label.clone()).collect(),
                })
            }
            AskKind::Slider { min, max, .. } => {
                let v = number(value, "a slider answer")?;
                if v < *min || v > *max {
                    return Err(Error::invalid_input(format!(
                        "slider answer {v} lies outside [{min}, {max}]"
                    )));
                }
                Ok(Answer::Slider { value: v })
            }
            AskKind::Range { min, max, .. } => {
                let [from, to] = pair_field(value, "a range answer")?;
                if from < *min || to > *max || from > to {
                    return Err(Error::invalid_input(format!(
                        "range answer [{from}, {to}] must satisfy {min} ≤ from ≤ to ≤ {max}"
                    )));
                }
                Ok(Answer::Range { from, to })
            }
        }
    }
}

fn find_option<'a>(options: &'a [AskOption], id: &str) -> Result<&'a AskOption> {
    options.iter().find(|o| o.id == id).ok_or_else(|| {
        Error::invalid_input(format!(
            "`{id}` is not an option of this ask; options: {}",
            options
                .iter()
                .map(|o| o.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })
}

fn parse_options(obj: &Map<String, Value>) -> Result<Vec<AskOption>> {
    let raw = obj
        .get("options")
        .ok_or_else(|| {
            Error::invalid_input("a choice ask needs `options`: 2–6 labels or {id,label,image} objects")
                .with_hint(r#"--ask '{"question":"Keep it?","type":"single_choice","options":["Keep","Delete"]}'"#)
        })?
        .as_array()
        .ok_or_else(|| Error::invalid_input("`ask.options` must be an array"))?;
    let mut out = Vec::with_capacity(raw.len());
    for (i, o) in raw.iter().enumerate() {
        let field = format!("ask.options[{i}]");
        let default_id = (i + 1).to_string();
        match o {
            Value::String(label) => out.push(AskOption {
                id: default_id,
                label: label.clone(),
                image: None,
            }),
            Value::Object(m) => {
                reject_unknown(m, &field, &["id", "label", "image"])?;
                let label = string_field(m, &field, "label")?
                    .ok_or_else(|| Error::invalid_input(format!("`{field}.label` is required")))?;
                let id = string_field(m, &field, "id")?.unwrap_or(default_id);
                let image = string_field(m, &field, "image")?.map(ImageRef::Path);
                out.push(AskOption { id, label, image });
            }
            other => {
                return Err(Error::invalid_input(format!(
                    "`{field}` must be a label string or an {{\"id\",\"label\",\"image\"}} object, got {}",
                    kind(other)
                )));
            }
        }
    }
    Ok(out)
}

fn check_options(options: &[AskOption], hop: ImageHop) -> Result<()> {
    let n = options.len();
    if n > limits::OPTIONS_MAX {
        return Err(Error::new(
            ErrorCode::TooManyOptions,
            format!(
                "the ask has {n} options; the limit is {}",
                limits::OPTIONS_MAX
            ),
        )
        .with_hint("ask a narrower question, or use a text ask")
        .with_details(json!({"limit": limits::OPTIONS_MAX, "actual": n})));
    }
    if n < limits::OPTIONS_MIN {
        return Err(Error::invalid_input(format!(
            "the ask has {n} option(s); a choice ask needs at least {}",
            limits::OPTIONS_MIN
        ))
        .with_hint("give 2–6 options, or use a text ask")
        .with_details(json!({"field": "ask.options", "limit": limits::OPTIONS_MIN, "actual": n})));
    }
    for (i, o) in options.iter().enumerate() {
        let valid_id = (1..=limits::OPTION_ID_MAX_CHARS).contains(&o.id.len())
            && o.id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if !valid_id {
            return Err(Error::invalid_input(format!(
                "`ask.options[{i}].id` is `{}`; ids are 1–32 of a-z 0-9 _ -",
                o.id
            )));
        }
        if options[..i].iter().any(|p| p.id == o.id) {
            return Err(Error::invalid_input(format!(
                "option id `{}` is used twice (options without an id default to their position, \"1\", \"2\", …)",
                o.id
            ))
            .with_hint("give every option a distinct id, or leave ids out")
            .with_details(json!({"field": format!("ask.options[{i}].id"), "duplicate_of": format!("ask.options[{}].id", options[..i].iter().position(|p| p.id == o.id).unwrap_or(0))})));
        }
        // Labels are what the Carbon reads, says or types: two that match
        // (ignoring case and surrounding spaces) cannot be told apart.
        let label = o.label.trim().to_lowercase();
        if let Some(j) = options[..i]
            .iter()
            .position(|p| p.label.trim().to_lowercase() == label)
        {
            return Err(Error::invalid_input(format!(
                "`ask.options[{i}].label` \"{}\" repeats `ask.options[{j}].label`; labels must be distinct (ignoring case and spaces)",
                o.label.trim()
            ))
            .with_hint("give every option a distinct label: the Carbon picks by what it reads, says or types")
            .with_details(json!({"field": format!("ask.options[{i}].label"), "duplicate_of": format!("ask.options[{j}].label")})));
        }
        check_text(
            &format!("ask.options[{i}].label"),
            &o.label,
            1,
            limits::OPTION_LABEL_MAX_CHARS,
            ErrorCode::TextTooLong,
            Lines::Single,
        )?;
        if let Some(img) = &o.image {
            check_image_ref(&format!("ask.options[{i}].image"), img, hop)?;
        }
    }
    Ok(())
}

fn check_scale(min: Num, max: Num, step: Num, unit: Option<&str>) -> Result<()> {
    if max <= min {
        return Err(Error::invalid_input(format!(
            "`ask.max` ({max}) must be greater than `ask.min` ({min})"
        )));
    }
    if step.get() <= 0.0 {
        return Err(Error::invalid_input(format!(
            "`ask.step` ({step}) must be greater than 0"
        )));
    }
    if step.get() > max.get() - min.get() {
        return Err(Error::invalid_input(format!(
            "`ask.step` ({step}) is larger than the scale ({min} to {max})"
        )));
    }
    if let Some(u) = unit {
        check_text(
            "ask.unit",
            u,
            0,
            limits::UNIT_MAX_CHARS,
            ErrorCode::TextTooLong,
            Lines::Single,
        )?;
    }
    Ok(())
}

fn describe(v: &Value) -> String {
    match v {
        Value::String(s) => format!("`{s}`"),
        other => kind(other).to_owned(),
    }
}

fn u32_field(v: &Value, field: &str) -> Result<u32> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| {
            Error::invalid_input(format!(
                "`{field}` must be a non-negative integer, got {}",
                describe(v)
            ))
        })
}

fn number(v: &Value, what: &str) -> Result<Num> {
    v.as_f64()
        .and_then(Num::new)
        .ok_or_else(|| Error::invalid_input(format!("{what} must be a number, got {}", kind(v))))
}

fn num_field(obj: &Map<String, Value>, key: &str) -> Result<Option<Num>> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => number(v, &format!("`ask.{key}`")).map(Some),
    }
}

fn pair_field(v: &Value, what: &str) -> Result<[Num; 2]> {
    match v.as_array().map(Vec::as_slice) {
        Some([a, b]) => Ok([number(a, what)?, number(b, what)?]),
        _ => Err(Error::invalid_input(format!(
            "{what} must be a [from, to] array of two numbers"
        ))),
    }
}
