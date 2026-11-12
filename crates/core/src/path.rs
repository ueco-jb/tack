use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::model::ItemId;
use crate::{Error, Result};

pub const MAX_TITLE_CHARS: usize = 200;

pub fn normalize_title(raw: &str) -> Result<String> {
    if raw.contains('/') {
        return Err(Error::invalid(format!(
            "title {raw:?} contains '/', which separates path segments"
        )));
    }
    if raw.chars().any(|c| c.is_control() && !c.is_whitespace()) {
        return Err(Error::invalid(format!(
            "title {raw:?} contains control characters"
        )));
    }
    let lowered = raw.to_lowercase().replace('_', " ");
    let title = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return Err(Error::invalid("title is empty"));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(Error::invalid(format!(
            "title is longer than {MAX_TITLE_CHARS} characters"
        )));
    }
    Ok(title)
}

pub fn name_of(title: &str) -> String {
    title.replace(' ', "_")
}

pub fn normalize_word(raw: &str) -> String {
    raw.to_lowercase()
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_")
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItemPath(Vec<String>);

impl ItemPath {
    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        let body = trimmed.strip_prefix('/').unwrap_or(trimmed);
        let body = body.strip_suffix('/').unwrap_or(body);
        if body.trim().is_empty() {
            return Err(Error::invalid("path is empty"));
        }
        let segments = body
            .split('/')
            .map(|segment| {
                normalize_title(segment).map_err(|e| {
                    Error::invalid(format!("invalid segment in path {raw:?}: {}", e.message))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self(segments))
    }

    pub fn segments(&self) -> &[String] {
        &self.0
    }

    pub fn title(&self) -> &str {
        self.0.last().expect("parse rejects empty paths")
    }

    pub fn parent(&self) -> Option<ItemPath> {
        (self.0.len() > 1).then(|| ItemPath(self.0[..self.0.len() - 1].to_vec()))
    }
}

impl fmt::Display for ItemPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, segment) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("/")?;
            }
            f.write_str(&name_of(segment))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Id(ItemId),
    Path(ItemPath),
}

impl FromStr for Target {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let trimmed = s.trim();
        if !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit()) {
            let id = trimmed
                .parse()
                .map_err(|_| Error::invalid(format!("id {trimmed} is out of range")))?;
            return Ok(Target::Id(ItemId(id)));
        }
        ItemPath::parse(trimmed).map(Target::Path)
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Id(id) => id.fmt(f),
            Target::Path(path) => {
                let text = path.to_string();
                if text.bytes().all(|b| b.is_ascii_digit()) {
                    f.write_str("/")?;
                }
                f.write_str(&text)
            }
        }
    }
}

impl Serialize for Target {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Target::Id(id) => serializer.serialize_i64(id.0),
            Target::Path(_) => serializer.serialize_str(&self.to_string()),
        }
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TargetVisitor;

        impl Visitor<'_> for TargetVisitor {
            type Value = Target;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an item id or a path")
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Target, E> {
                Ok(Target::Id(ItemId(v)))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Target, E> {
                i64::try_from(v)
                    .map(|v| Target::Id(ItemId(v)))
                    .map_err(|_| E::custom("id is out of range"))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Target, E> {
                v.parse().map_err(|e: Error| E::custom(e.message))
            }
        }

        deserializer.deserialize_any(TargetVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    #[test]
    fn titles_normalize_case_underscores_and_spacing() {
        assert_eq!(
            normalize_title("  Buy__Groceries  NOW ").unwrap(),
            "buy groceries now"
        );
        assert_eq!(name_of("buy groceries now"), "buy_groceries_now");
        assert_eq!(normalize_title("a\tb\nc").unwrap(), "a b c");
    }

    #[test]
    fn titles_reject_slash_empty_control_and_overlong() {
        assert_eq!(normalize_title("a/b").unwrap_err().kind, ErrorKind::Invalid);
        assert_eq!(normalize_title(" _ ").unwrap_err().kind, ErrorKind::Invalid);
        assert_eq!(
            normalize_title("a\u{7}b").unwrap_err().kind,
            ErrorKind::Invalid
        );
        assert!(normalize_title(&"ż".repeat(MAX_TITLE_CHARS)).is_ok());
        assert!(normalize_title(&"ż".repeat(MAX_TITLE_CHARS + 1)).is_err());
    }

    #[test]
    fn words_normalize_like_titles() {
        assert_eq!(normalize_word("Buy  Groceries"), "buy_groceries");
        assert_eq!(normalize_word("buy__groceries"), "buy_groceries");
        assert_eq!(normalize_word("weekly/Buy"), "weekly/buy");
    }

    #[test]
    fn parses_paths_with_spaces_or_underscores() {
        let a = ItemPath::parse("/Home/weekly/buy groceries/").unwrap();
        let b = ItemPath::parse("home/weekly/buy_groceries").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.to_string(), "home/weekly/buy_groceries");
        assert_eq!(a.segments()[2], "buy groceries");
    }

    #[test]
    fn rejects_empty_segments() {
        assert!(ItemPath::parse("home//weekly").is_err());
        assert!(ItemPath::parse("/").is_err());
    }

    #[test]
    fn bare_number_is_id_slash_number_is_path() {
        assert_eq!("12".parse::<Target>().unwrap(), Target::Id(ItemId(12)));
        let path: Target = "/2026".parse().unwrap();
        assert!(matches!(&path, Target::Path(p) if p.to_string() == "2026"));
        assert_eq!(path.to_string(), "/2026");
    }

    #[test]
    fn json_numbers_are_ids_strings_are_targets() {
        let id: Target = serde_json::from_str("7").unwrap();
        let path: Target = serde_json::from_str("\"home/weekly\"").unwrap();
        let string_id: Target = serde_json::from_str("\"7\"").unwrap();
        assert_eq!(id, Target::Id(ItemId(7)));
        assert_eq!(string_id, Target::Id(ItemId(7)));
        assert_eq!(serde_json::to_string(&path).unwrap(), "\"home/weekly\"");
    }
}
