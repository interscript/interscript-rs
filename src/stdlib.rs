//! `Interscript::Stdlib` parity: alias fragments, boundary semantics
//! and the stage-level functions (`upcase`, `separate`, `compose`, ...).

use crate::error::{Error, Result};
use unicode_normalization::UnicodeNormalization;

/// Zero-width and character-class primitives reachable from the ISC
/// grammar (`zw_primitive` rule) plus the Stdlib aliases they resolve
/// to. Values mirror `Interscript::Stdlib::ALIASES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prim {
    /// \b — Unicode-aware word boundary (Ruby \b is Unicode; \w is not).
    Boundary,
    /// \B
    NonWordBoundary,
    /// ^ — beginning of line.
    LineStart,
    /// $ — end of line.
    LineEnd,
    /// " " — a literal space (not re-only).
    Space,
    /// "none" — empty string (not re-only).
    None_,
    /// any_character — Ruby regex dot: any char except newline.
    AnyCharacter,
    /// \w — ASCII-only [A-Za-z0-9_].
    Word,
    /// \W
    NotWord,
    /// [a-zA-Z]
    Alpha,
    /// [^a-zA-Z]
    NotAlpha,
    /// \d — ASCII-only [0-9].
    Digit,
    /// \D
    NotDigit,
}

impl Prim {
    pub fn from_name(name: &str) -> Option<Prim> {
        Some(match name {
            "boundary" => Prim::Boundary,
            "non_boundary" | "non_word_boundary" => Prim::NonWordBoundary,
            "line_start" => Prim::LineStart,
            "line_end" => Prim::LineEnd,
            "space" => Prim::Space,
            "none" => Prim::None_,
            "any_character" => Prim::AnyCharacter,
            "word" => Prim::Word,
            "not_word" => Prim::NotWord,
            "alpha" => Prim::Alpha,
            "not_alpha" => Prim::NotAlpha,
            "digit" => Prim::Digit,
            "not_digit" => Prim::NotDigit,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Prim::Boundary => "boundary",
            Prim::NonWordBoundary => "non_word_boundary",
            Prim::LineStart => "line_start",
            Prim::LineEnd => "line_end",
            Prim::Space => "space",
            Prim::None_ => "none",
            Prim::AnyCharacter => "any_character",
            Prim::Word => "word",
            Prim::NotWord => "not_word",
            Prim::Alpha => "alpha",
            Prim::NotAlpha => "not_alpha",
            Prim::Digit => "digit",
            Prim::NotDigit => "not_digit",
        }
    }

    /// `Interscript::Stdlib.re_only_alias?` — can the primitive appear
    /// in a string / parallel (replacement) context? Only none/space.
    pub fn re_only(&self) -> bool {
        !matches!(self, Prim::None_ | Prim::Space)
    }

    /// Match as a character class at `ch`. `None` = zero-width (not a
    /// char matcher) or not applicable.
    pub fn matches_char(&self, ch: char) -> Option<bool> {
        Some(match self {
            Prim::Space => ch == ' ',
            Prim::None_ => false, // zero-width, handled before
            Prim::AnyCharacter => ch != '\n',
            Prim::Word => ch.is_ascii_alphanumeric() || ch == '_',
            Prim::NotWord => !(ch.is_ascii_alphanumeric() || ch == '_'),
            Prim::Alpha => ch.is_ascii_alphabetic(),
            Prim::NotAlpha => !ch.is_ascii_alphabetic(),
            Prim::Digit => ch.is_ascii_digit(),
            Prim::NotDigit => !ch.is_ascii_digit(),
            Prim::Boundary | Prim::NonWordBoundary | Prim::LineStart | Prim::LineEnd => return None,
        })
    }

    /// Zero-width assertions return tri-state: Some(true/false) decided
    /// at a position, None when the primitive is a char matcher.
    pub fn matches_zero_width(&self, chars: &[char], pos: usize) -> Option<bool> {
        Some(match self {
            Prim::Boundary => is_word_boundary(chars, pos),
            Prim::NonWordBoundary => !is_word_boundary(chars, pos),
            Prim::LineStart => pos == 0 || chars[pos - 1] == '\n',
            Prim::LineEnd => pos == chars.len() || chars[pos] == '\n',
            _ => return None,
        })
    }
}

/// Ruby `\b`: Unicode-aware. A position is a boundary when one side is
/// a word char (Unicode alphanumeric or underscore) and the other is
/// not (or is edge-of-string).
fn is_word_boundary(chars: &[char], pos: usize) -> bool {
    let before = pos > 0 && is_ruby_word_char(chars[pos - 1]);
    let after = pos < chars.len() && is_ruby_word_char(chars[pos]);
    before != after
}

/// Onigmo's UTF-8 word-char definition (what Ruby `\b` and `\B`
/// consult — NOT `\w`, which is ASCII-only): the Unicode Word
/// property = Alphabetic (L* + Nl) | Mark (Mn/Mc/Me) | Nd | Pc,
/// PLUS the explicit ZWNJ/ZWJ additions (U+200C, U+200D) Onigmo
/// special-cases — verified empirically against Ruby (a
/// letter+ZWNJ pair is NOT a boundary). `std` exposes no general
/// categories, hence unicode-general-category.
pub fn is_ruby_word_char(ch: char) -> bool {
    if matches!(ch, '\u{200C}' | '\u{200D}') {
        return true;
    }
    use unicode_general_category::{get_general_category, GeneralCategory as Gc};
    matches!(
        get_general_category(ch),
        Gc::UppercaseLetter
            | Gc::LowercaseLetter
            | Gc::TitlecaseLetter
            | Gc::ModifierLetter
            | Gc::OtherLetter
            | Gc::LetterNumber
            | Gc::DecimalNumber
            | Gc::NonspacingMark
            | Gc::SpacingMark
            | Gc::EnclosingMark
            | Gc::ConnectorPunctuation
    )
}

/// Stage-level function calls (`rule(:string_case_directive)`,
/// `separate_directive`, ...). `to`-side `upcase`/`downcase` in sub
/// rules are handled by the matcher, not here.
#[derive(Debug, Clone)]
pub struct Funcall {
    pub name: FuncallName,
    pub kwargs: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuncallName {
    Upcase,
    Downcase,
    TitleCase,
    Separate,
    Unseparate,
    Compose,
    Decompose,
}

impl FuncallName {
    pub fn from_name(name: &str) -> Option<FuncallName> {
        Some(match name {
            "upcase" => FuncallName::Upcase,
            "downcase" => FuncallName::Downcase,
            "title_case" => FuncallName::TitleCase,
            "separate" => FuncallName::Separate,
            "unseparate" => FuncallName::Unseparate,
            "compose" => FuncallName::Compose,
            "decompose" => FuncallName::Decompose,
            _ => return None,
        })
    }

    pub fn apply(&self, s: String, kwargs: &[(String, String)]) -> Result<String> {
        let kw = |name: &str| kwargs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        match self {
            FuncallName::Upcase => Ok(s.to_uppercase()),
            FuncallName::Downcase => match kw("word_separator") {
                Some(sep) => Ok(downcase_per_word(s, &sep)),
                None => Ok(s.to_lowercase()),
            },
            FuncallName::TitleCase => {
                let sep = kw("word_separator").unwrap_or_else(|| " ".to_string());
                Ok(title_case(s, &sep))
            }
            FuncallName::Separate => {
                let sep = kw("separator").unwrap_or_else(|| " ".to_string());
                let mut out = String::with_capacity(s.len() * 2);
                for (i, c) in s.chars().enumerate() {
                    if i > 0 {
                        out.push_str(&sep);
                    }
                    out.push(c);
                }
                Ok(out)
            }
            FuncallName::Unseparate => {
                let sep = kw("separator").unwrap_or_else(|| " ".to_string());
                Ok(s.split(&sep).collect::<String>())
            }
            // Ruby String#unicode_normalize(:nfc / :nfd)
            FuncallName::Compose => Ok(s.nfc().collect()),
            FuncallName::Decompose => Ok(s.nfd().collect()),
        }
    }
}

/// `Interscript::Stdlib::Functions.title_case`: `gsub(/^(.)/, &:upcase)`
/// then `gsub(/#{word_separator}(.)/, &:upcase)` — the block upcases
/// the WHOLE match (separator included), so the replacement for a
/// match `sep + ch` is `(sep + ch).to_uppercase`.
pub fn title_case(s: String, word_separator: &str) -> String {
    let out = upcase_first(s);
    if word_separator.is_empty() {
        return out;
    }
    let sep: Vec<char> = word_separator.chars().collect();
    let chars: Vec<char> = out.chars().collect();
    let mut result = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        if i + sep.len() < chars.len() && chars[i..i + sep.len()] == sep[..] {
            let mut m: String = sep.iter().collect();
            m.extend(chars[i + sep.len()].to_uppercase());
            result.push_str(&m);
            i += sep.len() + 1;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

/// `Interscript::Stdlib::Functions.downcase` with word_separator —
/// same whole-match downcasing.
pub fn downcase_per_word(s: String, word_separator: &str) -> String {
    let out = downcase_first(s);
    if word_separator.is_empty() {
        return out;
    }
    let sep: Vec<char> = word_separator.chars().collect();
    let chars: Vec<char> = out.chars().collect();
    let mut result = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        if i + sep.len() < chars.len() && chars[i..i + sep.len()] == sep[..] {
            let mut m: String = sep.iter().collect();
            m.extend(chars[i + sep.len()].to_lowercase());
            result.push_str(&m);
            i += sep.len() + 1;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

fn upcase_first(s: String) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => s,
    }
}

fn downcase_first(s: String) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => s,
    }
}

/// The funcalls that need external engines; unsupported in this slice.
pub fn unsupported_funcall(name: &str) -> Error {
    Error::Unsupported(format!(
        "funcall {name:?} (rababa/secryst need external ML engines; not in slice 1)"
    ))
}
