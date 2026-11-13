use std::collections::{BTreeMap, HashMap};

use jiff::Timestamp;

use crate::api::{CreateItem, ItemView, PatchItem};
use crate::model::{
    Category, Item, ItemId, Status, StatusId, Tag, default_tag_color, normalize_color,
    normalize_tag, normalize_tags, transition,
};
use crate::path::{ItemPath, MAX_TITLE_CHARS, Target, name_of};
use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    pub created: Vec<ItemId>,
    pub updated: Vec<ItemId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub item: Item,
    pub name: Box<str>,
    pub children: Vec<ItemId>,
    pub path: Box<str>,
    pub archived: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    nodes: HashMap<ItemId, Node>,
    roots: Vec<ItemId>,
    order: Vec<ItemId>,
    statuses: Vec<Status>,
    tags: BTreeMap<String, Option<String>>,
    next_item_id: i64,
}

impl Tree {
    pub fn new(
        mut statuses: Vec<Status>,
        items: Vec<Item>,
        tags: Vec<Tag>,
        next_item_id: i64,
    ) -> Result<Tree> {
        statuses.sort_by_key(|s| (s.position, s.id));
        for category in Category::ALL {
            if !statuses.iter().any(|s| s.category == category) {
                return Err(Error::internal(format!(
                    "no status with category {category}"
                )));
            }
        }
        let max_id = items.iter().map(|i| i.id.0).max().unwrap_or(0);
        let mut tree = Tree {
            nodes: HashMap::with_capacity(items.len()),
            roots: Vec::new(),
            order: Vec::new(),
            statuses,
            tags: tags.into_iter().map(|t| (t.name, t.color)).collect(),
            next_item_id: next_item_id.max(max_id + 1),
        };
        for item in items {
            if !tree.statuses.iter().any(|s| s.id == item.status) {
                return Err(Error::internal(format!(
                    "item {} has unknown status {}",
                    item.id, item.status
                )));
            }
            tree.register_tags(&item.tags);
            let id = item.id;
            if tree.nodes.insert(id, Node::detached(item)).is_some() {
                return Err(Error::internal(format!("duplicate item id {id}")));
            }
        }
        let ids: Vec<ItemId> = tree.nodes.keys().copied().collect();
        for &id in &ids {
            match tree.nodes[&id].item.parent {
                None => tree.roots.push(id),
                Some(parent) => tree
                    .nodes
                    .get_mut(&parent)
                    .ok_or_else(|| {
                        Error::internal(format!("item {id} has unknown parent {parent}"))
                    })?
                    .children
                    .push(id),
            }
        }
        tree.sort_siblings(None);
        for &id in &ids {
            tree.sort_siblings(Some(id));
        }
        for root in tree.roots.clone() {
            tree.refresh_subtree(root);
        }
        tree.rebuild_order();
        if tree.order.len() != tree.nodes.len() {
            return Err(Error::internal("items form a parent cycle"));
        }
        Ok(tree)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn next_item_id(&self) -> i64 {
        self.next_item_id
    }

    pub fn statuses(&self) -> &[Status] {
        &self.statuses
    }

    pub fn status(&self, id: StatusId) -> Result<&Status> {
        self.statuses
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| Error::internal(format!("unknown status id {id}")))
    }

    pub fn status_by_name(&self, name: &str) -> Result<&Status> {
        let wanted = name.trim().to_lowercase();
        self.statuses
            .iter()
            .find(|s| s.name == wanted)
            .ok_or_else(|| {
                let known: Vec<&str> = self.statuses.iter().map(|s| s.name.as_str()).collect();
                Error::invalid(format!(
                    "unknown status {name:?}; known: {}",
                    known.join(", ")
                ))
            })
    }

    pub fn first_status(&self, category: Category) -> Result<&Status> {
        self.statuses
            .iter()
            .find(|s| s.category == category)
            .ok_or_else(|| Error::internal(format!("no status with category {category}")))
    }

    pub fn tags(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(name, color)| Tag {
                name: name.clone(),
                color: color.clone(),
            })
            .collect()
    }

    pub fn set_tag_color(&mut self, name: &str, color: Option<&str>) -> Result<Tag> {
        let name = normalize_tag(name)?;
        let color = color.map(normalize_color).transpose()?;
        let slot = self
            .tags
            .get_mut(&name)
            .ok_or_else(|| Error::not_found(format!("no tag {name}")))?;
        *slot = color.clone();
        Ok(Tag { name, color })
    }

    pub fn remove_tag(&mut self, name: &str) -> Result<String> {
        let name = normalize_tag(name)?;
        if self.tags.remove(&name).is_none() {
            return Err(Error::not_found(format!("no tag {name}")));
        }
        for node in self.nodes.values_mut() {
            node.item.tags.retain(|t| *t != name);
        }
        Ok(name)
    }

    pub fn get(&self, id: ItemId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn node(&self, id: ItemId) -> Result<&Node> {
        self.nodes
            .get(&id)
            .ok_or_else(|| Error::not_found(format!("no item with id {id}")))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.order.iter().map(|id| &self.nodes[id])
    }

    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.nodes.values().map(|n| &n.item)
    }

    pub fn find(&self, target: &Target) -> Result<&Node> {
        match target {
            Target::Id(id) => self.node(*id),
            Target::Path(path) => {
                let mut cursor = None;
                for segment in path.segments() {
                    let found = self.find_child(cursor, &name_of(segment));
                    cursor = Some(found.ok_or_else(|| {
                        Error::not_found(format!("no item at {}", Target::Path(path.clone())))
                    })?);
                }
                cursor
                    .map(|id| &self.nodes[&id])
                    .ok_or_else(|| Error::invalid("path is empty"))
            }
        }
    }

    pub fn is_within(&self, id: ItemId, ancestor: ItemId) -> bool {
        let mut cursor = Some(id);
        while let Some(current) = cursor {
            if current == ancestor {
                return true;
            }
            cursor = self.nodes.get(&current).and_then(|n| n.item.parent);
        }
        false
    }

    pub fn view(&self, id: ItemId) -> Result<ItemView> {
        let node = self.node(id)?;
        let status = self.status(node.item.status)?;
        let item = &node.item;
        Ok(ItemView {
            id: item.id,
            parent_id: item.parent,
            path: node.path.to_string(),
            title: item.title.clone(),
            body: item.body.clone(),
            status: status.name.clone(),
            category: status.category,
            tags: item.tags.clone(),
            due: item.due,
            created_at: item.created_at,
            updated_at: item.updated_at,
            started_at: item.started_at,
            done_at: item.done_at,
            archived_at: item.archived_at,
            archived: node.archived,
        })
    }

    pub fn add(&mut self, req: &CreateItem, now: Timestamp) -> Result<Written> {
        let path = ItemPath::parse(&req.path)?;
        let status = match &req.status {
            Some(name) => self.status_by_name(name)?,
            None => self.first_status(Category::Todo)?,
        };
        let (status_id, category) = (status.id, status.category);
        let default_status = self.first_status(Category::Todo)?.id;
        let tags = normalize_tags(&req.tags)?;

        let segments = path.segments();
        let parents = &segments[..segments.len() - 1];
        let (cursor, found) = self.existing_prefix(parents);
        let updated = self.unarchive_ancestors(cursor, now);

        let (parent, mut created) =
            self.create_parents(cursor, &parents[found..], default_status, now);
        let id = self.take_id();
        let title = self.free_title(parent, path.title(), None);
        self.register_tags(&tags);
        self.insert_item(Item {
            id,
            parent,
            title,
            body: req.body.clone().unwrap_or_default(),
            status: status_id,
            tags,
            created_at: now,
            updated_at: now,
            started_at: (category == Category::Active).then_some(now),
            done_at: (category == Category::Done).then_some(now),
            archived_at: None,
            due: req.due,
        });
        created.push(id);
        self.rebuild_order();
        Ok(Written { created, updated })
    }

    pub fn update(&mut self, id: ItemId, patch: &PatchItem, now: Timestamp) -> Result<Written> {
        let old = self.node(id)?.item.clone();
        let mut new = old.clone();
        let mut missing_parents = Vec::new();
        let mut existing_parent = old.parent;

        if let Some(raw) = &patch.path {
            let path = ItemPath::parse(raw)?;
            let parents = &path.segments()[..path.segments().len() - 1];
            let (cursor, found) = self.existing_prefix(parents);
            if let Some(c) = cursor
                && self.is_within(c, id)
            {
                return Err(Error::conflict(format!(
                    "cannot move {} into itself or its own subtree",
                    self.nodes[&id].path
                )));
            }
            existing_parent = cursor;
            missing_parents = parents[found..].to_vec();
            new.parent = cursor;
            new.title = if missing_parents.is_empty() {
                self.free_title(cursor, path.title(), Some(id))
            } else {
                path.title().to_owned()
            };
        }
        if let Some(name) = &patch.status {
            let to = self.status_by_name(name)?;
            if to.id != old.status {
                let from = self.status(old.status)?;
                if to.category == Category::Done
                    && from.category != Category::Done
                    && !patch.force
                    && self.has_open_children(id)
                {
                    return Err(Error::conflict(format!(
                        "{} still has open items under it; force it to mark it done anyway",
                        self.nodes[&id].path
                    )));
                }
                transition(&mut new, from.category, to.category, now);
                new.status = to.id;
            }
        }
        if let Some(body) = &patch.body {
            new.body = body.clone();
        }
        if let Some(tags) = &patch.tags {
            new.tags = normalize_tags(tags)?;
        }
        if let Some(due) = patch.due {
            new.due = due;
        }
        match patch.archived {
            Some(true) if old.archived_at.is_none() => new.archived_at = Some(now),
            Some(false) => new.archived_at = None,
            _ => {}
        }
        let unarchive = patch.archived == Some(false) && self.nodes[&id].archived;
        if new == old && missing_parents.is_empty() && !unarchive {
            return Ok(Written {
                created: Vec::new(),
                updated: Vec::new(),
            });
        }

        let default_status = self.first_status(Category::Todo)?.id;
        let moved = existing_parent != old.parent || !missing_parents.is_empty();
        let mut updated = if moved || unarchive {
            self.unarchive_ancestors(existing_parent, now)
        } else {
            Vec::new()
        };
        let (parent, created) =
            self.create_parents(existing_parent, &missing_parents, default_status, now);
        if !missing_parents.is_empty() {
            new.parent = parent;
        }
        self.register_tags(&new.tags);
        new.updated_at = now;
        let restructured = old.parent != new.parent || old.title != new.title;
        let archive_changed = old.archived_at.is_some() != new.archived_at.is_some();
        let new_parent = new.parent;
        if restructured {
            self.remove_child(old.parent, id);
        }
        let node = self.nodes.get_mut(&id).expect("checked above");
        node.name = name_of(&new.title).into_boxed_str();
        node.item = new;
        if restructured {
            self.insert_child(new_parent, id);
        }
        if restructured || archive_changed {
            self.refresh_subtree(id);
        }
        if restructured {
            self.rebuild_order();
        }
        updated.push(id);
        Ok(Written { created, updated })
    }

    pub fn remove(&mut self, id: ItemId, recursive: bool) -> Result<Vec<ItemId>> {
        let node = self.node(id)?;
        if !node.children.is_empty() && !recursive {
            return Err(Error::conflict(format!(
                "{} has {} children; delete recursively to remove them too",
                node.path,
                node.children.len()
            )));
        }
        let parent = node.item.parent;
        let mut removed = Vec::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            removed.push(current);
            stack.extend(self.nodes[&current].children.iter().copied());
        }
        removed.reverse();
        self.remove_child(parent, id);
        for gone in &removed {
            self.nodes.remove(gone);
        }
        self.rebuild_order();
        Ok(removed)
    }

    fn existing_prefix(&self, segments: &[String]) -> (Option<ItemId>, usize) {
        let mut cursor = None;
        let mut found = 0;
        while found < segments.len()
            && let Some(next) = self.find_child(cursor, &name_of(&segments[found]))
        {
            cursor = Some(next);
            found += 1;
        }
        (cursor, found)
    }

    fn unarchive_ancestors(&mut self, start: Option<ItemId>, now: Timestamp) -> Vec<ItemId> {
        let mut changed = Vec::new();
        if !start.is_some_and(|s| self.nodes[&s].archived) {
            return changed;
        }
        let mut cursor = start;
        while let Some(id) = cursor {
            let node = self.nodes.get_mut(&id).expect("ancestors exist");
            if node.item.archived_at.take().is_some() {
                node.item.updated_at = now;
                changed.push(id);
            }
            cursor = node.item.parent;
        }
        if let Some(&top) = changed.last() {
            self.refresh_subtree(top);
        }
        changed
    }

    fn free_title(&self, parent: Option<ItemId>, title: &str, except: Option<ItemId>) -> String {
        let taken = |t: &str| {
            self.find_child(parent, &name_of(t))
                .is_some_and(|c| Some(c) != except)
        };
        if !taken(title) {
            return title.to_owned();
        }
        (2..)
            .map(|n| {
                let suffix = format!("-{n}");
                let keep = MAX_TITLE_CHARS - suffix.chars().count();
                format!("{}{suffix}", title.chars().take(keep).collect::<String>())
            })
            .find(|t| !taken(t))
            .expect("some suffix is free")
    }

    fn create_parents(
        &mut self,
        mut cursor: Option<ItemId>,
        titles: &[String],
        status: StatusId,
        now: Timestamp,
    ) -> (Option<ItemId>, Vec<ItemId>) {
        let mut created = Vec::with_capacity(titles.len());
        for title in titles {
            let id = self.take_id();
            self.insert_item(Item {
                id,
                parent: cursor,
                title: title.clone(),
                body: String::new(),
                status,
                tags: Vec::new(),
                created_at: now,
                updated_at: now,
                started_at: None,
                done_at: None,
                archived_at: None,
                due: None,
            });
            cursor = Some(id);
            created.push(id);
        }
        (cursor, created)
    }

    fn take_id(&mut self) -> ItemId {
        let id = ItemId(self.next_item_id);
        self.next_item_id += 1;
        id
    }

    fn insert_item(&mut self, item: Item) {
        let (id, parent) = (item.id, item.parent);
        self.nodes.insert(id, Node::detached(item));
        self.insert_child(parent, id);
        self.refresh_subtree(id);
    }

    fn register_tags(&mut self, tags: &[String]) {
        for tag in tags {
            self.tags
                .entry(tag.clone())
                .or_insert_with(|| Some(default_tag_color(tag).to_owned()));
        }
    }

    fn siblings(&self, parent: Option<ItemId>) -> &[ItemId] {
        match parent {
            None => &self.roots,
            Some(p) => &self.nodes[&p].children,
        }
    }

    fn siblings_mut(&mut self, parent: Option<ItemId>) -> &mut Vec<ItemId> {
        match parent {
            None => &mut self.roots,
            Some(p) => &mut self.nodes.get_mut(&p).expect("parent exists").children,
        }
    }

    fn sibling_key(&self, id: ItemId) -> (&str, ItemId) {
        (&self.nodes[&id].name, id)
    }

    fn sort_siblings(&mut self, parent: Option<ItemId>) {
        let mut list = std::mem::take(self.siblings_mut(parent));
        list.sort_by(|a, b| self.sibling_key(*a).cmp(&self.sibling_key(*b)));
        *self.siblings_mut(parent) = list;
    }

    fn insert_child(&mut self, parent: Option<ItemId>, id: ItemId) {
        let mut list = std::mem::take(self.siblings_mut(parent));
        let key = self.sibling_key(id);
        let pos = list
            .binary_search_by(|probe| self.sibling_key(*probe).cmp(&key))
            .unwrap_or_else(|pos| pos);
        list.insert(pos, id);
        *self.siblings_mut(parent) = list;
    }

    fn remove_child(&mut self, parent: Option<ItemId>, id: ItemId) {
        self.siblings_mut(parent).retain(|&c| c != id);
    }

    fn find_child(&self, parent: Option<ItemId>, name: &str) -> Option<ItemId> {
        self.siblings(parent)
            .iter()
            .copied()
            .find(|id| &*self.nodes[id].name == name)
    }

    fn has_open_children(&self, id: ItemId) -> bool {
        let mut stack = self.nodes[&id].children.clone();
        while let Some(c) = stack.pop() {
            let node = &self.nodes[&c];
            if node.item.archived_at.is_some() {
                continue;
            }
            if node.item.done_at.is_none() {
                return true;
            }
            stack.extend(node.children.iter().copied());
        }
        false
    }

    fn refresh_subtree(&mut self, start: ItemId) {
        let mut stack = vec![start];
        while let Some(id) = stack.pop() {
            let node = &self.nodes[&id];
            let archived = node.item.archived_at.is_some();
            let (path, archived) = match node.item.parent {
                None => (format!("/{}", node.name), archived),
                Some(p) => {
                    let parent = &self.nodes[&p];
                    (
                        format!("{}/{}", parent.path, node.name),
                        parent.archived || archived,
                    )
                }
            };
            let node = self.nodes.get_mut(&id).expect("id taken from the tree");
            node.path = path.into_boxed_str();
            node.archived = archived;
            stack.extend(node.children.iter().copied());
        }
    }

    // '.' and '-' sort before '/', so paths can't be string sorted
    fn rebuild_order(&mut self) {
        let mut order = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<ItemId> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            order.push(id);
            stack.extend(self.nodes[&id].children.iter().rev().copied());
        }
        self.order = order;
    }
}

impl Node {
    fn detached(item: Item) -> Node {
        Node {
            name: name_of(&item.title).into_boxed_str(),
            item,
            children: Vec::new(),
            path: Box::from(""),
            archived: false,
        }
    }
}
