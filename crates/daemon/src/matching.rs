//! Turns one final STT transcript into an answer value (BLUEPRINT §1.9.5):
//! normalize, try exact or fuzzy label matches, ordinals ("the second one"),
//! numerals ("option 2", "forty two"), and "and"-lists for multiple choice.
//!
//! The result is the raw value Peek.app would have reported for a click
//! (option id, array of ids, number, `[from, to]`), so it goes through the
//! same [`Ask::resolve_answer`](silicon_peek_client::schema::ask::Ask::resolve_answer)
//! checks. Ambiguity is never guessed: it is [`MatchOutcome::Unmatched`] and
//! the bubble stays open ("Didn't match an option — tap one or type").

use serde_json::{Value, json};
use silicon_peek_client::{
    num::Num,
    schema::ask::{Ask, AskKind, AskOption},
};

/// The outcome of matching one transcript.
#[derive(Clone, Debug, PartialEq)]
pub enum MatchOutcome {
    /// The transcript answers the ask with this raw value.
    Matched(Value),
    /// Something was said, but it does not answer the ask unambiguously.
    Unmatched,
    /// Nothing was said.
    Empty,
}

/// Matches `transcript` against `ask`.
#[must_use]
pub fn match_transcript(ask: &Ask, transcript: &str) -> MatchOutcome {
    let transcript = transcript.trim();
    if transcript.is_empty() || !transcript.chars().any(char::is_alphanumeric) {
        return MatchOutcome::Empty;
    }
    let outcome = match &ask.kind {
        AskKind::Text { max_length, .. } => {
            let max = usize::try_from(*max_length).unwrap_or(usize::MAX);
            Some(Value::String(transcript.chars().take(max).collect()))
        }
        AskKind::SingleChoice { options } => {
            let text = Text::new(transcript);
            match_single(&text, options).map(|i| Value::String(options[i].id.clone()))
        }
        AskKind::MultipleChoice { options, min, max } => {
            let text = Text::new(transcript);
            match_multiple(&text, options, *min, *max).map(|ids| {
                Value::Array(
                    ids.into_iter()
                        .map(|i| Value::String(options[i].id.clone()))
                        .collect(),
                )
            })
        }
        AskKind::Slider { min, max, step, .. } => {
            slider_value(&scale_values(transcript, *min, *max))
                .and_then(|v| snap(v, *min, *max, *step))
                .map(number_value)
        }
        AskKind::Range { min, max, step, .. } => {
            range_values(&scale_values(transcript, *min, *max), min.get(), max.get()).and_then(
                |(lo, hi)| match (snap(lo, *min, *max, *step), snap(hi, *min, *max, *step)) {
                    (Some(f), Some(t)) => Some(json!([number_value(f), number_value(t)])),
                    _ => None,
                },
            )
        }
    };
    outcome.map_or(MatchOutcome::Unmatched, MatchOutcome::Matched)
}

// ------------------------------------------------------------ normalization

/// One token: its matching form and the word as spoken.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Token {
    norm: String,
    orig: String,
}

/// A normalized transcript: clauses (split at punctuation) of tokens.
#[derive(Clone, Debug)]
struct Text {
    clauses: Vec<Vec<Token>>,
}

fn fold(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => "a",
        'ç' | 'č' => "c",
        'è' | 'é' | 'ê' | 'ë' | 'ē' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'ī' => "i",
        'ñ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => "o",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' => "u",
        'ý' | 'ÿ' => "y",
        'ß' => "ss",
        'œ' => "oe",
        'æ' => "ae",
        _ => return None,
    })
}

/// Lowercases, folds common accents, and keeps letters, digits, spaces,
/// apostrophes and decimal points inside numbers.
fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        let lower: String = c.to_lowercase().collect();
        for l in lower.chars() {
            if let Some(f) = fold(l) {
                out.push_str(f);
            } else if l.is_alphanumeric() {
                out.push(l);
            } else if l == '\'' || l == '’' {
                out.push('\'');
            } else if l == '.'
                && i > 0
                && chars[i - 1].is_ascii_digit()
                && chars.get(i + 1).is_some_and(char::is_ascii_digit)
            {
                out.push('.');
            } else {
                out.push(' ');
            }
        }
    }
    out
}

const NUMBER_WORDS: [(&str, u32); 28] = [
    ("zero", 0),
    ("one", 1),
    ("two", 2),
    ("three", 3),
    ("four", 4),
    ("five", 5),
    ("six", 6),
    ("seven", 7),
    ("eight", 8),
    ("nine", 9),
    ("ten", 10),
    ("eleven", 11),
    ("twelve", 12),
    ("thirteen", 13),
    ("fourteen", 14),
    ("fifteen", 15),
    ("sixteen", 16),
    ("seventeen", 17),
    ("eighteen", 18),
    ("nineteen", 19),
    ("twenty", 20),
    ("thirty", 30),
    ("forty", 40),
    ("fifty", 50),
    ("sixty", 60),
    ("seventy", 70),
    ("eighty", 80),
    ("ninety", 90),
];

fn number_word(w: &str) -> Option<u32> {
    NUMBER_WORDS.iter().find(|(n, _)| *n == w).map(|(_, v)| *v)
}

fn token(word: &str) -> Token {
    let orig = word.trim_matches('\'').to_owned();
    let norm = match number_word(&orig) {
        Some(n) => n.to_string(),
        None => orig.replace('\'', ""),
    };
    Token { norm, orig }
}

impl Text {
    fn new(raw: &str) -> Self {
        let mut clauses = Vec::new();
        for clause in raw
            .split([',', ';', '!', '?', ':', '\n'])
            .flat_map(split_sentences)
        {
            let tokens: Vec<Token> = clean(clause)
                .split_whitespace()
                .map(token)
                .filter(|t| !t.norm.is_empty())
                .collect();
            if !tokens.is_empty() {
                clauses.push(tokens);
            }
        }
        Self { clauses }
    }

    fn tokens(&self) -> impl Iterator<Item = &Token> {
        self.clauses.iter().flatten()
    }

    /// Tokens without filler words ("please", "I'd like", "option" …).
    fn content(&self) -> Vec<&Token> {
        self.tokens().filter(|t| !is_filler(t)).collect()
    }
}

/// Splits at sentence periods (not decimal points).
fn split_sentences(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'.' {
            let digit_before = i > 0 && bytes[i - 1].is_ascii_digit();
            let digit_after = bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
            if !(digit_before && digit_after) {
                out.push(&s[start..i]);
                start = i + 1;
            }
        }
    }
    out.push(&s[start..]);
    out
}

const FILLER: [&str; 44] = [
    "the", "um", "uh", "er", "erm", "hmm", "mm", "please", "i", "id", "ill", "im", "like", "want",
    "would", "choose", "pick", "select", "go", "with", "take", "say", "it", "that", "this",
    "option", "choice", "number", "lets", "okay", "ok", "so", "well", "just", "answer", "is", "my",
    "for", "me", "let", "us", "do", "prefer", "thanks",
];

const PURE_FILLER: [&str; 14] = [
    "um", "uh", "er", "erm", "hmm", "mm", "please", "okay", "ok", "so", "well", "the", "just",
    "thanks",
];

fn is_filler(t: &Token) -> bool {
    // "one" in "the second one" / "that one" is a pronoun, not a numeral.
    t.orig == "one" || FILLER.contains(&t.norm.as_str())
}

const NEGATORS: [&str; 13] = [
    "not", "dont", "never", "without", "except", "doesnt", "isnt", "wont", "neither", "nor",
    "cant", "besides", "shouldnt",
];

fn is_negator(t: &Token) -> bool {
    NEGATORS.contains(&t.norm.as_str())
}

/// Words that may sit between a negator and what it negates without
/// changing its scope: "don't *want to* delete", "not *the* second one",
/// "don't *you dare* delete it".
const BRIDGE: [&str; 17] = [
    "to", "you", "dare", "wanna", "need", "should", "really", "even", "ever", "a", "an", "we",
    "again", "ahead", "anymore", "go", "be",
];

fn is_bridge(t: &Token) -> bool {
    is_filler(t) || BRIDGE.contains(&t.norm.as_str())
}

/// Words that end a negator's scope: "alpha *but* not bravo", "don't keep
/// it, *actually* delete it".
const SCOPE_BREAKERS: [&str; 4] = ["but", "instead", "rather", "actually"];

/// Whether (and how) the token at `at` in `clause` is negated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Negation {
    /// No negator in scope.
    None,
    /// A negator right before it (only bridge words in between): "don't
    /// delete", "I don't want to delete it", "not the second one".
    Direct,
    /// A negator earlier in the clause with other words in between ("I don't
    /// know, keep it" without the comma): its scope cannot be resolved, so
    /// the transcript is not matched (never guessed).
    Remote,
}

fn negation_at(clause: &[Token], at: usize) -> Negation {
    let mut only_bridges = true;
    for (i, t) in clause[..at].iter().enumerate().rev() {
        if is_negator(t) {
            return if only_bridges {
                Negation::Direct
            } else {
                Negation::Remote
            };
        }
        if t.norm == "but"
            && clause[..i]
                .iter()
                .any(|p| ALL_WORDS.contains(&p.norm.as_str()))
        {
            // "all but the second one", "everything but bravo": an exclusion.
            return if only_bridges {
                Negation::Direct
            } else {
                Negation::Remote
            };
        }
        if SCOPE_BREAKERS.contains(&t.norm.as_str()) {
            return Negation::None;
        }
        if !is_bridge(t) {
            only_bridges = false;
        }
    }
    Negation::None
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Similarity in `[0, 1]` (1 = identical).
fn similarity(a: &str, b: &str) -> f64 {
    let max = a.chars().count().max(b.chars().count());
    if max == 0 {
        return 1.0;
    }
    let d = levenshtein(a, b);
    1.0 - f64::from(u32::try_from(d).unwrap_or(u32::MAX))
        / f64::from(u32::try_from(max).unwrap_or(u32::MAX))
}

fn singular(w: &str) -> &str {
    if w.len() > 4 && w.ends_with("es") {
        &w[..w.len() - 2]
    } else if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") {
        &w[..w.len() - 1]
    } else {
        w
    }
}

fn token_eq(spoken: &str, label: &str) -> bool {
    if spoken == label || singular(spoken) == singular(label) {
        return true;
    }
    let short = spoken.chars().count().min(label.chars().count());
    short >= 5 && levenshtein(spoken, label) <= 1
}

// ----------------------------------------------------------------- choices

fn label_tokens(o: &AskOption) -> Vec<String> {
    clean(&o.label)
        .split_whitespace()
        .map(|w| token(w).norm)
        .filter(|w| !w.is_empty())
        .collect()
}

/// The option id as an extra spoken alias (`keep_all` → "keep all"), when it
/// is a word and not a positional number.
fn id_tokens(o: &AskOption) -> Option<Vec<String>> {
    if o.id.chars().all(|c| c.is_ascii_digit()) || o.id.len() < 2 {
        return None;
    }
    let t: Vec<String> =
        o.id.split(['_', '-'])
            .filter(|p| !p.is_empty())
            .map(|p| token(p).norm)
            .collect();
    (!t.is_empty()).then_some(t)
}

const YES: [&str; 9] = [
    "yes",
    "yeah",
    "yep",
    "yup",
    "sure",
    "correct",
    "affirmative",
    "absolutely",
    "definitely",
];
const NO: [&str; 5] = ["no", "nope", "nah", "negative", "nay"];

fn yes_no(word: &str) -> Option<&'static str> {
    if YES.contains(&word) {
        Some("yes")
    } else if NO.contains(&word) {
        Some("no")
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug)]
struct Hit {
    option: usize,
    len: usize,
    negation: Negation,
    clause: usize,
    start: usize,
}

impl Hit {
    fn negated(&self) -> bool {
        self.negation == Negation::Direct
    }
}

fn label_hits(text: &Text, options: &[AskOption]) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (oi, o) in options.iter().enumerate() {
        let mut forms = vec![label_tokens(o)];
        forms.extend(id_tokens(o));
        for form in forms.iter().filter(|f| !f.is_empty()) {
            for (ci, clause) in text.clauses.iter().enumerate() {
                if clause.len() < form.len() {
                    continue;
                }
                for start in 0..=clause.len() - form.len() {
                    let matched = form
                        .iter()
                        .zip(&clause[start..start + form.len()])
                        .all(|(l, t)| token_eq(&t.norm, l));
                    if matched {
                        hits.push(Hit {
                            option: oi,
                            len: form.len(),
                            negation: negation_at(clause, start),
                            clause: ci,
                            start,
                        });
                    }
                }
            }
        }
    }
    hits
}

/// Among positive hits, keeps only those not covered by a longer hit of
/// another option ("keep all" beats "keep").
fn dominant_options(hits: &[Hit]) -> Vec<usize> {
    let positive: Vec<&Hit> = hits
        .iter()
        .filter(|h| h.negation == Negation::None)
        .collect();
    let mut out: Vec<usize> = Vec::new();
    for h in &positive {
        let covered = positive.iter().any(|o| {
            o.option != h.option
                && o.clause == h.clause
                && o.len > h.len
                && o.start <= h.start
                && o.start + o.len >= h.start + h.len
        });
        if !covered && !out.contains(&h.option) {
            out.push(h.option);
        }
    }
    out
}

const ORDINALS: [(&str, usize); 12] = [
    ("first", 1),
    ("second", 2),
    ("third", 3),
    ("fourth", 4),
    ("fifth", 5),
    ("sixth", 6),
    ("1st", 1),
    ("2nd", 2),
    ("3rd", 3),
    ("4th", 4),
    ("5th", 5),
    ("6th", 6),
];

fn ordinal_of(t: &Token, n: usize) -> Option<usize> {
    if t.norm == "last" {
        Some(n)
    } else {
        ORDINALS.iter().find(|(w, _)| *w == t.norm).map(|(_, i)| *i)
    }
}

/// 1-based positions named by ordinals ("second", "3rd", "last"), each with
/// its negation ("not the second one").
fn ordinal_refs(text: &Text, n: usize) -> Vec<(usize, Negation)> {
    let mut out: Vec<(usize, Negation)> = Vec::new();
    for clause in &text.clauses {
        for (at, t) in clause.iter().enumerate() {
            if let Some(i) = ordinal_of(t, n)
                && !out.iter().any(|(o, _)| *o == i)
            {
                out.push((i, negation_at(clause, at)));
            }
        }
    }
    out
}

/// What the ordinals of a single-choice transcript select: `Ok(Some(index))`,
/// `Ok(None)` when there is no ordinal, `Err(())` when they do not answer
/// unambiguously (several, out of range, or a negation that is not the
/// complement of exactly one of two options).
fn single_from_positions(refs: &[(usize, Negation)], n: usize) -> Result<Option<usize>, ()> {
    if refs.is_empty() {
        return Ok(None);
    }
    if refs.iter().any(|(_, g)| *g == Negation::Remote) {
        return Err(());
    }
    let positive: Vec<usize> = refs
        .iter()
        .filter(|(_, g)| *g == Negation::None)
        .map(|(i, _)| *i)
        .collect();
    match positive.as_slice() {
        [i] if *i >= 1 && *i <= n => return Ok(Some(i - 1)),
        [] => {}
        _ => return Err(()),
    }
    // Only negated positions: "not the second one" of two options is the
    // other one; with more options it says nothing definite.
    match refs {
        [(i, Negation::Direct)] if n == 2 && (*i == 1 || *i == 2) => Ok(Some(2 - i)),
        _ => Err(()),
    }
}

/// A spoken number or letter as a 1-based position, when it is in range.
fn as_index(t: &Token, n: usize) -> Option<usize> {
    if let Ok(v) = t.norm.parse::<usize>() {
        return (1..=n).contains(&v).then_some(v);
    }
    let mut chars = t.norm.chars();
    match (chars.next(), chars.next()) {
        (Some(c @ 'a'..='f'), None) => {
            let v = usize::from(u8::try_from(c).unwrap_or(b'a') - b'a') + 1;
            (v <= n).then_some(v)
        }
        _ => None,
    }
}

/// What a numeral or letter names: the whole utterance ("two", "b") or the
/// word after "option"/"number"/"choice"/"letter" ("option 3"), with its
/// negation ("not option two"). Returns a 1-based option position.
///
/// "Option N" is always positional. A bare number is first read as a label:
/// with options "1 day", "3 days", "7 days", "three" is "3 days" (not the
/// third option) and "seven" is "7 days". A bare number that no label
/// contains is positional only when no label contains a number at all;
/// otherwise it is ambiguous (`Err`).
fn numeral_ref(
    text: &Text,
    options: &[AskOption],
) -> std::result::Result<Option<(usize, Negation)>, ()> {
    let n = options.len();
    for clause in &text.clauses {
        for (at, w) in clause.windows(2).enumerate() {
            if matches!(
                w[0].norm.as_str(),
                "option" | "number" | "choice" | "letter"
            ) && let Some(i) = as_index(&w[1], n)
            {
                return Ok(Some((i, negation_at(clause, at))));
            }
        }
    }
    // Only pure fillers are dropped here: "that one" is deictic, not "1".
    let content: Vec<&Token> = text
        .tokens()
        .filter(|t| !PURE_FILLER.contains(&t.norm.as_str()))
        .collect();
    let [only] = content.as_slice() else {
        return Ok(None);
    };
    if only.norm.parse::<f64>().is_err() {
        // A letter ("b") or a word is positional or nothing, as before.
        return Ok(as_index(only, n).map(|i| (i, Negation::None)));
    }
    let labels: Vec<Vec<String>> = options.iter().map(label_tokens).collect();
    let containing: Vec<usize> = labels
        .iter()
        .enumerate()
        .filter(|(_, l)| l.len() > 1 && l.contains(&only.norm))
        .map(|(i, _)| i)
        .collect();
    match containing.as_slice() {
        [i] => return Ok(Some((i + 1, Negation::None))),
        [] => {}
        _ => return Err(()),
    }
    let numeric = |w: &String| w.parse::<f64>().is_ok();
    if labels.iter().flatten().any(numeric) {
        // Labels carry numbers but none is this one: "3" of "1 day / 7 days /
        // 30 days" may mean the third option or a mishearing. Never guessed.
        return Err(());
    }
    Ok(as_index(only, n).map(|i| (i, Negation::None)))
}

fn exact_option(text: &Text, options: &[AskOption]) -> Option<usize> {
    let content: Vec<String> = text.content().iter().map(|t| t.norm.clone()).collect();
    if content.is_empty() {
        return None;
    }
    let joined = content.join(" ");
    let mut found = options.iter().enumerate().filter(|(_, o)| {
        label_tokens(o).join(" ") == joined || id_tokens(o).is_some_and(|t| t.join(" ") == joined)
    });
    let first = found.next().map(|(i, _)| i);
    if found.next().is_some() { None } else { first }
}

fn yes_no_option(text: &Text, options: &[AskOption]) -> Option<usize> {
    let content = text.content();
    let [only] = content.as_slice() else {
        return None;
    };
    let said = yes_no(&only.norm)?;
    let mut matching = options
        .iter()
        .enumerate()
        .filter(|(_, o)| matches!(label_tokens(o).as_slice(), [w] if yes_no(w) == Some(said)));
    let first = matching.next().map(|(i, _)| i);
    if matching.next().is_some() {
        None
    } else {
        first
    }
}

fn fuzzy_option(text: &Text, options: &[AskOption]) -> Option<usize> {
    let content: Vec<String> = text.content().iter().map(|t| t.norm.clone()).collect();
    let spoken = content.join(" ");
    if spoken.chars().count() < 3 {
        return None;
    }
    let mut scored: Vec<(usize, f64)> = options
        .iter()
        .enumerate()
        .map(|(i, o)| (i, similarity(&spoken, &label_tokens(o).join(" "))))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    match scored.as_slice() {
        [(i, best), rest @ ..]
            if *best >= 0.75 && rest.first().is_none_or(|(_, s)| *s <= best - 0.15) =>
        {
            Some(*i)
        }
        _ => None,
    }
}

/// Single choice: exact → yes/no → labels → ordinal → numeral → the
/// complement of a negated option (2 options) → fuzzy.
///
/// Negation is honoured wherever it applies ("don't delete", "I don't want
/// to delete it", "not the second one"); a negation whose scope is unclear,
/// or one that leaves more than one candidate, is never guessed.
fn match_single(text: &Text, options: &[AskOption]) -> Option<usize> {
    let n = options.len();
    if let Some(i) = exact_option(text, options).or_else(|| yes_no_option(text, options)) {
        return Some(i);
    }
    let hits = label_hits(text, options);
    if hits.iter().any(|h| h.negation == Negation::Remote) {
        return None;
    }
    let dominant = dominant_options(&hits);
    match dominant.as_slice() {
        [one] => return Some(*one),
        [] => {}
        _ => return None,
    }
    match single_from_positions(&ordinal_refs(text, n), n) {
        Ok(Some(i)) => return Some(i),
        Ok(None) => {}
        Err(()) => return None,
    }
    match numeral_ref(text, options) {
        Ok(Some(found)) => {
            return single_from_positions(&[found], n).ok().flatten();
        }
        Ok(None) => {}
        Err(()) => return None,
    }
    let negated: Vec<usize> = hits
        .iter()
        .filter(|h| h.negated())
        .map(|h| h.option)
        .collect();
    if n == 2 && !negated.is_empty() && negated.iter().all(|o| *o == negated[0]) {
        return Some(1 - negated[0]);
    }
    if !negated.is_empty() || text.tokens().any(is_negator) {
        // A negation that did not resolve to one option: fuzzy matching
        // would only guess.
        return None;
    }
    fuzzy_option(text, options)
}

const ALL_WORDS: [&str; 5] = ["all", "everything", "every", "both", "each"];
const NONE_WORDS: [&str; 3] = ["none", "nothing", "neither"];

/// Splits "alpha and charlie, plus delta" into items.
fn list_items(text: &Text) -> Vec<Text> {
    let mut items = Vec::new();
    for clause in &text.clauses {
        let mut cur = Vec::new();
        for t in clause {
            if matches!(t.norm.as_str(), "and" | "plus" | "also" | "or") {
                if !cur.is_empty() {
                    items.push(Text {
                        clauses: vec![std::mem::take(&mut cur)],
                    });
                }
            } else {
                cur.push(t.clone());
            }
        }
        if !cur.is_empty() {
            items.push(Text { clauses: vec![cur] });
        }
    }
    items
}

/// Multiple choice: "all"/"none", label hits anywhere (minus negated ones),
/// plus each "and"-separated item resolved like a single choice. The count
/// must fit `[min, max]`.
///
/// Exclusions ("everything except the first", "all but bravo") and
/// negations whose scope is unclear are never guessed: they are unmatched.
fn match_multiple(text: &Text, options: &[AskOption], min: u32, max: u32) -> Option<Vec<usize>> {
    let n = options.len();
    let content = text.content();
    let only_all = !content.is_empty()
        && content
            .iter()
            .all(|t| ALL_WORDS.contains(&t.norm.as_str()) || t.norm == "of" || t.norm == "them");
    let only_none = !content.is_empty()
        && content
            .iter()
            .all(|t| NONE_WORDS.contains(&t.norm.as_str()) || t.norm == "of" || t.norm == "them");
    let chosen: Vec<usize> = if only_all {
        (0..n).collect()
    } else if only_none {
        Vec::new()
    } else {
        let has_all = content.iter().any(|t| ALL_WORDS.contains(&t.norm.as_str()));
        let excludes = content.iter().any(|t| is_negator(t) || t.norm == "but");
        if has_all && excludes {
            return None;
        }
        let hits = label_hits(text, options);
        if hits.iter().any(|h| h.negation == Negation::Remote) {
            return None;
        }
        let mut negated: Vec<usize> = hits
            .iter()
            .filter(|h| h.negated())
            .map(|h| h.option)
            .collect();
        let mut picked: Vec<usize> = dominant_options(&hits);
        let ordinals = ordinal_refs(text, n);
        for (i, g) in &ordinals {
            if *i == 0 || *i > n {
                continue;
            }
            match g {
                Negation::None => {
                    if !picked.contains(&(i - 1)) {
                        picked.push(i - 1);
                    }
                }
                Negation::Direct => negated.push(i - 1),
                Negation::Remote => return None,
            }
        }
        for item in list_items(text) {
            if item.tokens().any(|t| ordinal_of(t, n).is_some()) {
                continue; // resolved above, with its negation
            }
            let has_negator = item.tokens().any(is_negator);
            let found = match numeral_ref(&item, options) {
                Err(()) => return None,
                Ok(Some((i, g))) => {
                    if has_negator || g != Negation::None {
                        // "not option two" inside a list: never guessed.
                        return None;
                    }
                    Some(i - 1)
                }
                Ok(None) if has_negator => None,
                Ok(None) => exact_option(&item, options).or_else(|| fuzzy_option(&item, options)),
            };
            if let Some(i) = found
                && !picked.contains(&i)
            {
                picked.push(i);
            }
        }
        picked.retain(|i| !negated.contains(i));
        if picked.is_empty() {
            return None;
        }
        picked.sort_unstable();
        picked
    };
    let count = u32::try_from(chosen.len()).unwrap_or(u32::MAX);
    (count >= min && count <= max).then_some(chosen)
}

// ------------------------------------------------------------------ scales

/// One number said for a slider or range.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Said {
    /// A plain value ("42", "max", "halfway").
    Value(f64),
    /// An inclusive upper bound ("at most 30", "no more than 30", "up to
    /// 30", "30 or less", "max 30").
    AtMost(f64),
    /// An inclusive lower bound ("at least 20", "no less than 20", "20 or
    /// more", "min 20").
    AtLeast(f64),
    /// A strict bound ("under 30", "more than 20"): a range limit, but no
    /// single slider value.
    Below(f64),
    /// See [`Said::Below`].
    Above(f64),
}

/// Builds a bound from the number said after (or before) its words.
type Bound = fn(f64) -> Said;

const MORE: [&str; 6] = ["more", "higher", "greater", "above", "over", "bigger"];
const LESS: [&str; 6] = ["less", "fewer", "lower", "below", "under", "smaller"];

/// Numbers, bounds and keywords ("max", "min", "half") in order of
/// appearance. "Most" and "least" are never values on their own: "at most
/// 30" is a bound of 30 (not the maximum), and "most likely 40" is 40.
fn scale_values(raw: &str, min: Num, max: Num) -> Vec<Said> {
    let lo = min.get();
    let hi = max.get();
    let mut out = Vec::new();
    let cleaned = clean_scale(raw);
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let number = |at: usize| {
        (at < words.len())
            .then(|| parse_number_at(&words, at))
            .flatten()
    };
    let word = |at: usize| words.get(at).copied().unwrap_or("");
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        // Prefix bounds: "at most N", "no more than N", "up to N", "max N",
        // "more than N", "under N".
        let prefix: Option<(usize, Bound)> = match (w, word(i + 1)) {
            ("at", "most") | ("up", "to") | ("max" | "maximum", "of") => Some((2, Said::AtMost)),
            ("at", "least") | ("min" | "minimum", "of") => Some((2, Said::AtLeast)),
            ("no" | "not", m) if MORE.contains(&m) => {
                Some((if word(i + 2) == "than" { 3 } else { 2 }, Said::AtMost))
            }
            ("no" | "not", l) if LESS.contains(&l) => {
                Some((if word(i + 2) == "than" { 3 } else { 2 }, Said::AtLeast))
            }
            ("max" | "maximum", _) => Some((1, Said::AtMost)),
            ("min" | "minimum", _) => Some((1, Said::AtLeast)),
            (m, "than") if MORE.contains(&m) => Some((2, Said::Above)),
            (l, "than") if LESS.contains(&l) => Some((2, Said::Below)),
            ("above" | "over", _) => Some((1, Said::Above)),
            ("below" | "under", _) => Some((1, Said::Below)),
            _ => None,
        };
        if let Some((skip, bound)) = prefix
            && let Some((v, used)) = number(i + skip)
        {
            out.push(bound(v));
            i += skip + used;
            continue;
        }
        if let Some((v, used)) = number(i) {
            // Suffix bounds: "N or less", "N or more", "N at most", "N max".
            let after = i + used;
            let said = match (word(after), word(after + 1)) {
                ("or", l) if LESS.contains(&l) => Some((2, Said::AtMost(v))),
                ("or", m) if MORE.contains(&m) => Some((2, Said::AtLeast(v))),
                ("at", "most") => Some((2, Said::AtMost(v))),
                ("at", "least") => Some((2, Said::AtLeast(v))),
                ("max" | "maximum" | "tops", _) => Some((1, Said::AtMost(v))),
                ("minimum", _) => Some((1, Said::AtLeast(v))),
                _ => None,
            };
            if let Some((extra, bound)) = said {
                out.push(bound);
                i = after + extra;
            } else {
                out.push(Said::Value(v));
                i = after;
            }
            continue;
        }
        match w {
            "max" | "maximum" | "highest" | "top" | "full" => out.push(Said::Value(hi)),
            "min" | "minimum" | "lowest" | "bottom" => out.push(Said::Value(lo)),
            "half" | "halfway" | "middle" | "mid" | "midpoint" => {
                out.push(Said::Value(lo + (hi - lo) / 2.0));
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// A slider's value: the first plain value, or the one inclusive bound said
/// alone ("at most 30" → 30). Anything else with a bound in it (two bounds,
/// a strict "more than 20") names no single value and is unmatched.
fn slider_value(said: &[Said]) -> Option<f64> {
    let bounded = said.iter().any(|s| !matches!(s, Said::Value(_)));
    match said {
        [Said::AtMost(v) | Said::AtLeast(v)] => Some(*v),
        _ if bounded => None,
        [Said::Value(v), ..] => Some(*v),
        _ => None,
    }
}

/// A range's `(from, to)`: two plain values in either order, or bounds with
/// the ask's own limits on the open side ("at least 20" → `[20, max]`, "at
/// most 30" → `[min, 30]`, "at least 20 and at most 60" → `[20, 60]`).
/// Values mixed with bounds, or repeated bounds, are unmatched.
fn range_values(said: &[Said], min: f64, max: f64) -> Option<(f64, f64)> {
    let mut lower: Vec<f64> = Vec::new();
    let mut upper: Vec<f64> = Vec::new();
    let mut values: Vec<f64> = Vec::new();
    for s in said {
        match *s {
            Said::Value(v) => values.push(v),
            Said::AtLeast(v) | Said::Above(v) => lower.push(v),
            Said::AtMost(v) | Said::Below(v) => upper.push(v),
        }
    }
    if lower.is_empty() && upper.is_empty() {
        return match values.as_slice() {
            [a, b, ..] => Some(if a <= b { (*a, *b) } else { (*b, *a) }),
            _ => None,
        };
    }
    if !values.is_empty() || lower.len() > 1 || upper.len() > 1 {
        return None;
    }
    let from = lower.first().copied().unwrap_or(min);
    let to = upper.first().copied().unwrap_or(max);
    (from <= to).then_some((from, to))
}

/// Keeps digits, signs, decimal points and letters; separates "10-30" into a
/// range and "42%" into a number.
fn clean_scale(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    for (i, c) in chars.iter().enumerate() {
        let lower: String = c.to_lowercase().collect();
        for l in lower.chars() {
            let prev = i.checked_sub(1).map(|j| chars[j]);
            let next = chars.get(i + 1).copied();
            if l.is_alphanumeric() {
                out.push(fold(l).map_or(l, |f| f.chars().next().unwrap_or(l)));
            } else if l == '.'
                && prev.is_some_and(|p| p.is_ascii_digit())
                && next.is_some_and(|n| n.is_ascii_digit())
            {
                out.push('.');
            } else if (l == '-' || l == '−')
                && next.is_some_and(|n| n.is_ascii_digit())
                && prev.is_none_or(|p| p.is_whitespace() || p == '(')
            {
                out.push(' ');
                out.push('-');
            } else if l == ','
                && prev.is_some_and(|p| p.is_ascii_digit())
                && next.is_some_and(|n| n.is_ascii_digit())
            {
                // Thousands separator: "1,000".
            } else {
                out.push(' ');
            }
        }
    }
    out
}

/// Parses a number starting at `words[i]`: digits (`-3.5`), or number words
/// ("minus forty two point five", "one hundred and five", "a half").
fn parse_number_at(words: &[&str], at: usize) -> Option<(f64, usize)> {
    let first = words[at];
    if let Ok(v) = first.parse::<f64>()
        && v.is_finite()
    {
        return Some((v, 1));
    }
    let (sign, start) = if matches!(first, "minus" | "negative") && at + 1 < words.len() {
        (-1.0, at + 1)
    } else {
        (1.0, at)
    };
    if sign < 0.0
        && let Ok(v) = words[start].parse::<f64>()
    {
        return Some((-v, 2));
    }
    let mut total: f64 = 0.0;
    let mut current: f64 = 0.0;
    let mut pos = start;
    let mut any = false;
    let mut after_scale = false;
    let mut last_unit: Option<u32> = None;
    while pos < words.len() {
        let word = words[pos];
        if let Some(v) = number_word(word) {
            // "twenty one" continues; "thirty ten" does not.
            if let Some(prev) = last_unit
                && !(prev >= 20 && prev % 10 == 0 && v < 10)
            {
                break;
            }
            current += f64::from(v);
            any = true;
            after_scale = false;
            last_unit = Some(v);
        } else if word == "hundred" && any {
            current *= 100.0;
            after_scale = true;
            last_unit = None;
        } else if word == "thousand" && any {
            total += current * 1000.0;
            current = 0.0;
            after_scale = true;
            last_unit = None;
        } else if word == "and"
            && after_scale
            && pos + 1 < words.len()
            && number_word(words[pos + 1]).is_some()
        {
            // "one hundred and five"
        } else {
            break;
        }
        pos += 1;
    }
    if !any {
        return None;
    }
    let mut value = total + current;
    if pos + 1 < words.len() && words[pos] == "point" {
        let mut frac = String::new();
        let mut next = pos + 1;
        while next < words.len() {
            match number_word(words[next]).filter(|v| *v < 10) {
                Some(digit) => frac.push_str(&digit.to_string()),
                None => break,
            }
            next += 1;
        }
        if !frac.is_empty()
            && let Ok(f) = format!("0.{frac}").parse::<f64>()
        {
            value += f;
            pos = next;
        }
    }
    Some((sign * value, pos - at))
}

fn decimals(step: f64) -> i32 {
    let mut d = 0;
    let mut s = step;
    while d < 6 && (s - s.round()).abs() > 1e-9 {
        s *= 10.0;
        d += 1;
    }
    d
}

/// Snaps `v` to `min + k × step` and checks the bounds.
fn snap(value: f64, min: Num, max: Num, step: Num) -> Option<f64> {
    let (low, high, stride) = (min.get(), max.get(), step.get());
    if !value.is_finite() || stride <= 0.0 {
        return None;
    }
    let steps = ((value - low) / stride).round();
    let places = decimals(stride).max(decimals(low));
    let scale = 10f64.powi(places);
    let snapped = ((low + steps * stride) * scale).round() / scale;
    let eps = stride / 1000.0;
    (snapped >= low - eps && snapped <= high + eps).then(|| snapped.clamp(low, high))
}

fn number_value(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9.0e15 {
        #[allow(clippy::cast_possible_truncation)] // integral and in range, checked above
        let i = v as i64;
        json!(i)
    } else {
        json!(v)
    }
}
