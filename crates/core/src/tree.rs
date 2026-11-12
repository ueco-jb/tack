use std::collections::HashMap;

use jiff::Timestamp;

use crate::api::{CreateItem, CreateStatus, ItemView, PatchItem, PatchStatus};
use crate::model::{
    Category, Item, ItemId, Status, StatusId, normalize_status_name, normalize_tags, transition,
};
use crate::path::{ItemPath, Target, name_of};
use crate::{Error, Result};

pub const MAX_LABEL_CHARS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Updated {
    pub changed: bool,
    pub created: Vec<ItemId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub item: Item,
    pub name: Box<str>,
    pub children: Vec<ItemId>,
    pub path: Box<str>,
    pub depth: u32,
    pub open: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    nodes: HashMap<ItemId, Node>,
    roots: Vec<ItemId>,
    order: Vec<ItemId>,
    statuses: Vec<Status>,
    next_item_id: i64,
}

impl Tree {
    pub fn new(mut statuses: Vec<Status>, items: Vec<Item>, next_item_id: i64) -> Result<Tree> {
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
            next_item_id: next_item_id.max(max_id + 1),
        };
        for item in items {
            if !tree.statuses.iter().any(|s| s.id == item.status) {
                return Err(Error::internal(format!(
                    "item {} has unknown status {}",
                    item.id, item.status
                )));
            }
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
                    let found = self.find_live_child(cursor, &name_of(segment));
                    cursor = Some(found.ok_or_else(|| {
                        Error::not_found(format!("no open item at {}", Target::Path(path.clone())))
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
            tags: item.tags.clone(),
            created_at: item.created_at,
            updated_at: item.updated_at,
            started_at: item.started_at,
            done_at: item.done_at,
        })
    }

    pub fn add(&mut self, req: &CreateItem, now: Timestamp) -> Result<Vec<ItemId>> {
        let path = ItemPath::parse(&req.path)?;
        let status = match &req.status {
            Some(name) => self.status_by_name(name)?,
            None => self.first_status(Category::Todo)?,
        };
        let (status_id, category) = (status.id, status.category);
        let default_status = self.first_status(Category::Todo)?.id;
        let tags = normalize_tags(&req.tags)?;

        let segments = path.segments();
        let (cursor, found) = self.existing_prefix(segments);
        if found == segments.len() {
            return Err(Error::conflict(format!(
                "an open item already exists at {path}"
            )));
        }
        let (parent, mut created) = self.create_parents(
            cursor,
            &segments[found..segments.len() - 1],
            default_status,
            now,
        );
        let id = self.take_id();
        self.insert_item(Item {
            id,
            parent,
            title: path.title().to_owned(),
            body: req.body.clone().unwrap_or_default(),
            status: status_id,
            tags,
            created_at: now,
            updated_at: now,
            started_at: (category == Category::Active).then_some(now),
            done_at: (category == Category::Done).then_some(now),
        });
        created.push(id);
        self.rebuild_order();
        Ok(created)
    }

    pub fn update(&mut self, id: ItemId, patch: &PatchItem, now: Timestamp) -> Result<Updated> {
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
            new.title = path.title().to_owned();
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
        if new == old && missing_parents.is_empty() {
            return Ok(Updated {
                changed: false,
                created: Vec::new(),
            });
        }

        let reappears = new.title != old.title
            || new.parent != old.parent
            || (old.done_at.is_some() && new.done_at.is_none());
        if new.done_at.is_none()
            && reappears
            && missing_parents.is_empty()
            && let Some(other) = self.find_live_child(new.parent, &name_of(&new.title))
            && other != id
        {
            return Err(Error::conflict(format!(
                "an open item already exists at {}",
                self.nodes[&other].path
            )));
        }
        let default_status = self.first_status(Category::Todo)?.id;

        let (parent, created) =
            self.create_parents(existing_parent, &missing_parents, default_status, now);
        if !missing_parents.is_empty() {
            new.parent = parent;
        }
        new.updated_at = now;
        let restructured = old.parent != new.parent || old.title != new.title;
        let done_changed = old.done_at.is_some() != new.done_at.is_some();
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
        if restructured || done_changed {
            self.refresh_subtree(id);
        }
        if restructured {
            self.rebuild_order();
        }
        Ok(Updated {
            changed: true,
            created,
        })
    }

    fn existing_prefix(&self, segments: &[String]) -> (Option<ItemId>, usize) {
        let mut cursor = None;
        let mut found = 0;
        while found < segments.len()
            && let Some(next) = self.find_live_child(cursor, &name_of(&segments[found]))
        {
            cursor = Some(next);
            found += 1;
        }
        (cursor, found)
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

    pub fn add_status(&mut self, req: &CreateStatus) -> Result<StatusId> {
        let name = normalize_status_name(&req.name)?;
        if self.statuses.iter().any(|s| s.name == name) {
            return Err(Error::conflict(format!("status {name} already exists")));
        }
        let label = normalize_label(req.label.as_deref().unwrap_or(&name))?;
        let id = StatusId(self.statuses.iter().map(|s| s.id.0).max().unwrap_or(0) + 1);
        let position = self.statuses.iter().map(|s| s.position).max().unwrap_or(0) + 1;
        self.statuses.push(Status {
            id,
            name,
            label,
            category: req.category,
            position,
        });
        Ok(id)
    }

    pub fn update_status(&mut self, name: &str, patch: &PatchStatus) -> Result<StatusId> {
        let old = self.status_by_name(name)?;
        let mut new = old.clone();
        if let Some(raw) = &patch.name {
            let renamed = normalize_status_name(raw)?;
            if renamed != old.name && self.statuses.iter().any(|s| s.name == renamed) {
                return Err(Error::conflict(format!("status {renamed} already exists")));
            }
            new.name = renamed;
        }
        if let Some(label) = &patch.label {
            new.label = normalize_label(label)?;
        }
        if let Some(category) = patch.category
            && category != old.category
        {
            if self.status_in_use(old.id) {
                return Err(Error::conflict(format!(
                    "status {} is in use; move its items to another status before changing its category",
                    old.name
                )));
            }
            self.ensure_not_last_of_category(old)?;
            new.category = category;
        }
        if let Some(position) = patch.position {
            new.position = position;
        }
        let id = new.id;
        let slot = self
            .statuses
            .iter_mut()
            .find(|s| s.id == id)
            .expect("found by name above");
        *slot = new;
        self.statuses.sort_by_key(|s| (s.position, s.id));
        Ok(id)
    }

    pub fn remove_status(&mut self, name: &str) -> Result<StatusId> {
        let status = self.status_by_name(name)?;
        if self.status_in_use(status.id) {
            return Err(Error::conflict(format!(
                "status {} is in use; move its items to another status first",
                status.name
            )));
        }
        self.ensure_not_last_of_category(status)?;
        let id = status.id;
        self.statuses.retain(|s| s.id != id);
        Ok(id)
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

    fn find_live_child(&self, parent: Option<ItemId>, name: &str) -> Option<ItemId> {
        self.siblings(parent).iter().copied().find(|id| {
            let node = &self.nodes[id];
            &*node.name == name && node.item.done_at.is_none()
        })
    }

    fn has_open_children(&self, id: ItemId) -> bool {
        self.nodes[&id]
            .children
            .iter()
            .any(|c| self.nodes[c].item.done_at.is_none())
    }

    fn status_in_use(&self, id: StatusId) -> bool {
        self.nodes.values().any(|n| n.item.status == id)
    }

    fn ensure_not_last_of_category(&self, status: &Status) -> Result<()> {
        let count = self
            .statuses
            .iter()
            .filter(|s| s.category == status.category)
            .count();
        if count <= 1 {
            return Err(Error::conflict(format!(
                "{} is the only status with category {}",
                status.name, status.category
            )));
        }
        Ok(())
    }

    fn refresh_subtree(&mut self, start: ItemId) {
        let mut stack = vec![start];
        while let Some(id) = stack.pop() {
            let node = &self.nodes[&id];
            let done = node.item.done_at.is_some();
            let (path, depth, open) = match node.item.parent {
                None => (node.name.to_string(), 0, !done),
                Some(p) => {
                    let parent = &self.nodes[&p];
                    (
                        format!("{}/{}", parent.path, node.name),
                        parent.depth + 1,
                        parent.open && !done,
                    )
                }
            };
            let node = self.nodes.get_mut(&id).expect("id taken from the tree");
            node.path = path.into_boxed_str();
            node.depth = depth;
            node.open = open;
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
            depth: 0,
            open: true,
        }
    }
}

fn normalize_label(raw: &str) -> Result<String> {
    let label = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.is_empty() || label.chars().count() > MAX_LABEL_CHARS {
        return Err(Error::invalid(format!(
            "status label must be 1 to {MAX_LABEL_CHARS} characters"
        )));
    }
    Ok(label)
}
