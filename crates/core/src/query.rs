use crate::model::{ItemId, StatusId, normalize_tag};
use crate::path::normalize_word;
use crate::tree::Tree;
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Is {
    #[default]
    Listed,
    Open,
    Done,
    Archived,
    All,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub words: Vec<String>,
    pub without: Vec<String>,
    pub status: Option<String>,
    pub tags: Vec<String>,
    pub is: Is,
}

const KEYS: [&str; 3] = ["is", "status", "tag"];

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
            } else if let Some((key, value)) = token.split_once(':')
                && KEYS.contains(&key.to_lowercase().as_str())
            {
                if value.is_empty() {
                    continue;
                }
                match key.to_lowercase().as_str() {
                    "is" => q.is = parse_is(value)?,
                    "status" => q.status = Some(value.to_lowercase()),
                    _ => q.tags.push(normalize_tag(value)?),
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
        "archived" => Ok(Is::Archived),
        "all" => Ok(Is::All),
        other => Err(Error::invalid(format!(
            "unknown is:{other}; expected is:open, is:done, is:archived or is:all"
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
        let status = query
            .status
            .as_deref()
            .map(|s| self.status_by_name(s).map(|s| s.id))
            .transpose()?;
        let (words, typed_statuses) = self.split_status_words(&query.words);
        Ok(self
            .iter()
            .filter(|node| {
                let done = node.item.done_at.is_some();
                let is = match query.is {
                    Is::Listed => !node.archived,
                    Is::Open => !node.archived && !done,
                    Is::Done => !node.archived && done,
                    Is::Archived => node.archived,
                    Is::All => true,
                };
                is && words.iter().all(|w| matches_word(&node.path, w))
                    && !query.without.iter().any(|w| matches_word(&node.path, w))
                    && status.is_none_or(|s| node.item.status == s)
                    && (typed_statuses.is_empty() || typed_statuses.contains(&node.item.status))
                    && query
                        .tags
                        .iter()
                        .all(|t| node.item.tags.binary_search(t).is_ok())
            })
            .map(|node| node.item.id)
            .collect())
    }

    fn split_status_words<'q>(&self, words: &'q [String]) -> (Vec<&'q str>, Vec<StatusId>) {
        let phrases: Vec<(Vec<String>, StatusId)> = self
            .statuses()
            .iter()
            .flat_map(|s| {
                [s.name.clone(), normalize_word(&s.label)]
                    .map(|p| (p.split('_').map(str::to_owned).collect(), s.id))
            })
            .collect();
        let mut rest = Vec::new();
        let mut found = Vec::new();
        let mut i = 0;
        'words: while i < words.len() {
            for (parts, id) in &phrases {
                let joined = parts.join("_");
                let spread = words.len() - i >= parts.len()
                    && parts.iter().zip(&words[i..]).all(|(p, w)| p == w);
                if words[i] == joined || spread {
                    found.push(*id);
                    i += if words[i] == joined { 1 } else { parts.len() };
                    continue 'words;
                }
            }
            rest.push(words[i].as_str());
            i += 1;
        }
        (rest, found)
    }
}

fn matches_word(path: &str, word: &str) -> bool {
    match word.strip_suffix('/') {
        Some(exact) if word.starts_with('/') => path
            .strip_prefix(exact)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/')),
        _ if word.starts_with('/') => path.starts_with(word),
        _ => path.contains(word),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_words_operators_and_negation() {
        let q = Query::parse(r#"weekly -work +food status:Doing is:all "buy  groceries""#).unwrap();
        assert_eq!(q.words, ["weekly", "buy_groceries"]);
        assert_eq!(q.without, ["work"]);
        assert_eq!(q.tags, ["food"]);
        assert_eq!(q.status.as_deref(), Some("doing"));
        assert_eq!(q.is, Is::All);
    }

    #[test]
    fn unfinished_operators_are_ignored_while_typing() {
        assert_eq!(Query::parse("status: is:").unwrap(), Query::default());
    }

    #[test]
    fn non_operator_colons_bangs_and_quoted_plus_are_words() {
        let q = Query::parse(r#"fix:wifi !urgent "+food""#).unwrap();
        assert_eq!(q.words, ["fix:wifi", "!urgent", "+food"]);
        assert!(q.tags.is_empty());
    }

    #[test]
    fn invalid_is_value_is_an_error() {
        assert!(Query::parse("is:maybe").is_err());
    }
}
