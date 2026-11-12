use std::fmt;
use std::str::FromStr;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemId(pub i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StatusId(pub i64);

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for StatusId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Todo,
    Active,
    Done,
}

impl Category {
    pub const ALL: [Category; 3] = [Category::Todo, Category::Active, Category::Done];

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Todo => "todo",
            Category::Active => "active",
            Category::Done => "done",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Category {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "todo" => Ok(Category::Todo),
            "active" => Ok(Category::Active),
            "done" => Ok(Category::Done),
            other => Err(Error::invalid(format!(
                "unknown category {other:?}; expected todo, active or done"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub id: StatusId,
    pub name: String,
    pub label: String,
    pub category: Category,
    pub position: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,
    pub parent: Option<ItemId>,
    pub title: String,
    pub body: String,
    pub status: StatusId,
    pub tags: Vec<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub done_at: Option<Timestamp>,
    pub archived_at: Option<Timestamp>,
    pub due: Option<jiff::civil::Date>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    pub color: Option<String>,
}

pub const MAX_TAG_CHARS: usize = 32;

pub fn normalize_tag(raw: &str) -> Result<String> {
    let tag = raw.trim().trim_start_matches('+').to_lowercase();
    if tag.is_empty() {
        return Err(Error::invalid("tag is empty"));
    }
    if tag
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '+' | ',' | ':' | '"'))
    {
        return Err(Error::invalid(format!(
            "tag {raw:?} may not contain whitespace, '+', ',', ':' or quotes"
        )));
    }
    if tag.chars().count() > MAX_TAG_CHARS {
        return Err(Error::invalid(format!(
            "tag {raw:?} is longer than {MAX_TAG_CHARS} characters"
        )));
    }
    Ok(tag)
}

pub fn normalize_tags<S: AsRef<str>>(raw: &[S]) -> Result<Vec<String>> {
    let mut tags = raw
        .iter()
        .map(|t| normalize_tag(t.as_ref()))
        .collect::<Result<Vec<_>>>()?;
    tags.sort();
    tags.dedup();
    Ok(tags)
}

pub const TAG_COLORS: [&str; 10] = [
    "red", "orange", "yellow", "green", "teal", "blue", "indigo", "purple", "pink", "gray",
];

pub fn default_tag_color(name: &str) -> &'static str {
    let hash = name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    TAG_COLORS[(hash % TAG_COLORS.len() as u64) as usize]
}

pub const MAX_COLOR_CHARS: usize = 32;

pub fn normalize_color(raw: &str) -> Result<String> {
    let color = raw.trim();
    if color.is_empty()
        || color.chars().count() > MAX_COLOR_CHARS
        || color.chars().any(char::is_control)
    {
        return Err(Error::invalid(format!(
            "color must be 1 to {MAX_COLOR_CHARS} characters"
        )));
    }
    Ok(color.to_owned())
}

pub const MAX_STATUS_NAME_CHARS: usize = 32;

pub fn normalize_status_name(raw: &str) -> Result<String> {
    let name = raw.trim().to_lowercase();
    if name.is_empty()
        || name.len() > MAX_STATUS_NAME_CHARS
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(Error::invalid(format!(
            "status name {raw:?} must be 1 to {MAX_STATUS_NAME_CHARS} characters of a to z, 0 to 9 or _"
        )));
    }
    Ok(name)
}

pub fn whole_seconds(t: Timestamp) -> Timestamp {
    Timestamp::from_second(t.as_second()).expect("truncating keeps the timestamp in range")
}

pub fn transition(item: &mut Item, from: Category, to: Category, now: Timestamp) {
    if to == Category::Active && from != Category::Active && item.started_at.is_none() {
        item.started_at = Some(now);
    }
    match (from, to) {
        (Category::Done, Category::Done) => {}
        (_, Category::Done) => item.done_at = Some(now),
        _ => item.done_at = None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> Item {
        Item {
            id: ItemId(1),
            parent: None,
            title: "x".into(),
            body: String::new(),
            status: StatusId(1),
            tags: vec![],
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
            started_at: None,
            done_at: None,
            archived_at: None,
            due: None,
        }
    }

    #[test]
    fn transition_stamps_and_clears() {
        let t1 = Timestamp::from_second(10).unwrap();
        let t2 = Timestamp::from_second(20).unwrap();
        let mut it = item();
        transition(&mut it, Category::Todo, Category::Active, t1);
        assert_eq!((it.started_at, it.done_at), (Some(t1), None));
        transition(&mut it, Category::Active, Category::Done, t2);
        assert_eq!((it.started_at, it.done_at), (Some(t1), Some(t2)));
        transition(&mut it, Category::Done, Category::Active, t2);
        assert_eq!((it.started_at, it.done_at), (Some(t1), None));
    }

    #[test]
    fn done_to_done_keeps_original_done_time() {
        let t1 = Timestamp::from_second(10).unwrap();
        let mut it = item();
        transition(&mut it, Category::Todo, Category::Done, t1);
        transition(
            &mut it,
            Category::Done,
            Category::Done,
            Timestamp::from_second(99).unwrap(),
        );
        assert_eq!(it.done_at, Some(t1));
        assert_eq!(it.started_at, None);
    }

    #[test]
    fn tags_normalize_sort_dedup() {
        assert_eq!(
            normalize_tags(&["+Food", "net", "food"]).unwrap(),
            vec!["food", "net"]
        );
        assert!(normalize_tag("two words").is_err());
    }
}
