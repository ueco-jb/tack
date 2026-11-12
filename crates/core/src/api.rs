use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::ErrorKind;
use crate::model::{Category, ItemId, Priority, Status};
use crate::path::Target;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemView {
    pub id: ItemId,
    pub parent_id: Option<ItemId>,
    pub path: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub category: Category,
    pub priority: Priority,
    pub tags: Vec<String>,
    pub depth: u32,
    pub open: bool,
    pub children_open: usize,
    pub children_total: usize,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub done_at: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemList {
    pub items: Vec<ItemView>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateItem {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Target>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Created {
    pub item: ItemView,
    pub created_ancestors: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent: Option<Option<Target>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deleted {
    pub deleted: Vec<ItemId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusList {
    pub statuses: Vec<Status>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateStatus {
    pub name: String,
    pub category: Category,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub kind: ErrorKind,
    pub message: String,
}

impl From<&crate::Error> for ErrorBody {
    fn from(e: &crate::Error) -> Self {
        Self {
            error: ErrorDetail {
                kind: e.kind,
                message: e.message.clone(),
            },
        }
    }
}

fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_distinguishes_absent_null_and_value() {
        let absent: PatchItem = serde_json::from_str("{}").unwrap();
        let null: PatchItem = serde_json::from_str(r#"{"parent": null}"#).unwrap();
        let value: PatchItem = serde_json::from_str(r#"{"parent": "home"}"#).unwrap();
        assert_eq!(absent.parent, None);
        assert_eq!(null.parent, Some(None));
        assert!(matches!(value.parent, Some(Some(Target::Path(_)))));
        assert_eq!(serde_json::to_string(&null).unwrap(), r#"{"parent":null}"#);
        assert_eq!(serde_json::to_string(&absent).unwrap(), "{}");
    }

    #[test]
    fn priority_out_of_range_is_rejected_at_the_boundary() {
        assert!(serde_json::from_str::<PatchItem>(r#"{"priority": 4}"#).is_err());
    }
}
