use crate::model::{ItemId, Priority, normalize_tag};
use crate::path::{Target, normalize_word};
use crate::tree::Tree;
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Is {
    #[default]
    Open,
    Done,
    All,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub words: Vec<String>,
    pub without: Vec<String>,
    pub under: Option<Target>,
    pub status: Option<String>,
    pub tags: Vec<String>,
    pub priority: Option<Priority>,
    pub is: Is,
}

const KEYS: [&str; 5] = ["is", "under", "status", "tag", "prio"];

impl Query {
    pub fn parse(input: &str) -> Result<Query> {
        let mut q = Query::default();
        for (token, quoted) in tokenize(input) {
            if let Some(word) = token.strip_prefix('-').filter(|w| !w.is_empty()) {
                push_word(&mut q.without, word);
            } else if quoted {
                push_word(&mut q.words, &token);
            } else if let Some(tag) = token.strip_prefix('+').filter(|t| !t.is_empty()) {
                q.tags.push(normalize_tag(tag)?);
            } else if let Some(p) = token
                .strip_prefix('!')
                .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            {
                q.priority = Some(p.parse()?);
            } else if let Some((key, value)) = token.split_once(':')
                && KEYS.contains(&key.to_lowercase().as_str())
            {
                if value.is_empty() {
                    continue;
                }
                match key.to_lowercase().as_str() {
                    "is" => q.is = parse_is(value)?,
                    "under" => q.under = Some(value.parse()?),
                    "status" => q.status = Some(value.to_lowercase()),
                    "tag" => q.tags.push(normalize_tag(value)?),
                    _ => q.priority = Some(value.parse()?),
                }
            } else {
                push_word(&mut q.words, &token);
            }
        }
        Ok(q)
    }
}

fn push_word(list: &mut Vec<String>, raw: &str) {
    let word = normalize_word(raw);
    if !word.is_empty() {
        list.push(word);
    }
}

fn parse_is(value: &str) -> Result<Is> {
    match value.to_lowercase().as_str() {
        "open" => Ok(Is::Open),
        "done" => Ok(Is::Done),
        "all" => Ok(Is::All),
        other => Err(Error::invalid(format!(
            "unknown is:{other}; expected is:open, is:done or is:all"
        ))),
    }
}

fn tokenize(input: &str) -> Vec<(String, bool)> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut in_quotes = false;
    for c in input.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                quoted = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push((std::mem::take(&mut current), quoted));
                }
                quoted = false;
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push((current, quoted));
    }
    tokens
}

impl Tree {
    pub fn list(&self, query: &Query) -> Result<Vec<ItemId>> {
        let under = query
            .under
            .as_ref()
            .map(|t| self.find(t).map(|n| n.item.id))
            .transpose()?;
        let status = query
            .status
            .as_deref()
            .map(|s| self.status_by_name(s).map(|s| s.id))
            .transpose()?;
        Ok(self
            .iter()
            .filter(|node| {
                let is = match query.is {
                    Is::Open => node.open,
                    Is::Done => !node.open,
                    Is::All => true,
                };
                is && query.words.iter().all(|w| node.path.contains(w.as_str()))
                    && !query.without.iter().any(|w| node.path.contains(w.as_str()))
                    && under.is_none_or(|a| self.is_within(node.item.id, a))
                    && status.is_none_or(|s| node.item.status == s)
                    && query
                        .tags
                        .iter()
                        .all(|t| node.item.tags.binary_search(t).is_ok())
                    && query.priority.is_none_or(|p| node.item.priority == p)
            })
            .map(|node| node.item.id)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_words_operators_and_negation() {
        let q = Query::parse(
            r#"weekly -work +food !2 status:Progress under:home is:all "buy  groceries""#,
        )
        .unwrap();
        assert_eq!(q.words, ["weekly", "buy_groceries"]);
        assert_eq!(q.without, ["work"]);
        assert_eq!(q.tags, ["food"]);
        assert_eq!(q.priority, Some(Priority::new(2).unwrap()));
        assert_eq!(q.status.as_deref(), Some("progress"));
        assert_eq!(q.under, Some("home".parse().unwrap()));
        assert_eq!(q.is, Is::All);
    }

    #[test]
    fn unfinished_operators_are_ignored_while_typing() {
        assert_eq!(Query::parse("status: under:").unwrap(), Query::default());
    }

    #[test]
    fn non_operator_colons_bangs_and_quoted_plus_are_words() {
        let q = Query::parse(r#"fix:wifi !urgent "+food""#).unwrap();
        assert_eq!(q.words, ["fix:wifi", "!urgent", "+food"]);
        assert!(q.tags.is_empty());
    }

    #[test]
    fn invalid_values_are_errors() {
        assert!(Query::parse("!7").is_err());
        assert!(Query::parse("is:maybe").is_err());
    }
}
