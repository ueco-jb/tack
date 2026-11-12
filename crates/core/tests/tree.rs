use jiff::Timestamp;
use tack_core::ErrorKind;
use tack_core::api::{CreateItem, CreateStatus, PatchItem, PatchStatus};
use tack_core::model::{Category, ItemId, Status, StatusId};
use tack_core::path::Target;
use tack_core::query::Query;
use tack_core::tree::{Tree, Updated};

fn seed_statuses() -> Vec<Status> {
    let status = |id, name: &str, category, position| Status {
        id: StatusId(id),
        name: name.into(),
        label: name.into(),
        category,
        position,
    };
    vec![
        status(1, "todo", Category::Todo, 1),
        status(2, "progress", Category::Active, 2),
        status(3, "done", Category::Done, 3),
    ]
}

struct Harness {
    tree: Tree,
    clock: i64,
}

impl Harness {
    fn new() -> Self {
        Self {
            tree: Tree::new(seed_statuses(), vec![], 1).unwrap(),
            clock: 1_000,
        }
    }

    fn now(&mut self) -> Timestamp {
        self.clock += 1;
        Timestamp::from_second(self.clock).unwrap()
    }

    fn try_add(&mut self, title: &str) -> tack_core::Result<ItemId> {
        let now = self.now();
        let created = self.tree.add(
            &CreateItem {
                path: title.into(),
                ..Default::default()
            },
            now,
        )?;
        Ok(*created.last().unwrap())
    }

    fn add(&mut self, title: &str) -> ItemId {
        self.try_add(title).unwrap()
    }

    fn try_patch(&mut self, target: &str, patch: PatchItem) -> tack_core::Result<Updated> {
        let id = self.id(target);
        let now = self.now();
        self.tree.update(id, &patch, now)
    }

    fn set_status(&mut self, target: &str, status: &str) -> tack_core::Result<Updated> {
        self.try_patch(
            target,
            PatchItem {
                status: Some(status.into()),
                ..Default::default()
            },
        )
    }

    fn id(&self, target: &str) -> ItemId {
        self.tree
            .find(&target.parse::<Target>().unwrap())
            .unwrap()
            .item
            .id
    }

    fn paths(&self, query: &str) -> Vec<String> {
        self.tree
            .list(&Query::parse(query).unwrap())
            .unwrap()
            .into_iter()
            .map(|id| self.tree.get(id).unwrap().path.to_string())
            .collect()
    }

    fn assert_matches_rebuild(&self) {
        let rebuilt = Tree::new(
            self.tree.statuses().to_vec(),
            self.tree.items().cloned().collect(),
            self.tree.next_item_id(),
        )
        .unwrap();
        assert_eq!(self.tree, rebuilt);
    }
}

#[test]
fn adding_a_path_creates_missing_ancestors_once() {
    let mut h = Harness::new();
    h.add("Home/Weekly/Buy Groceries");
    h.add("home/weekly/renovations/kitchen");
    assert_eq!(
        h.paths(""),
        [
            "home",
            "home/weekly",
            "home/weekly/buy_groceries",
            "home/weekly/renovations",
            "home/weekly/renovations/kitchen",
        ]
    );
    let err = h.try_add("home/weekly/buy_groceries").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
}

#[test]
fn a_rejected_add_changes_nothing() {
    let mut h = Harness::new();
    h.add("home/weekly/buy groceries");
    let before = h.tree.clone();
    assert!(h.try_add("home/weekly/buy groceries").is_err());
    assert_eq!(h.tree, before);
}

#[test]
fn a_done_item_frees_its_name_for_the_next_one() {
    let mut h = Harness::new();
    h.add("home/weekly/buy groceries");
    h.set_status("home/weekly/buy groceries", "done").unwrap();
    let second = h.add("home/weekly/buy groceries");
    assert_eq!(h.id("home/weekly/buy_groceries"), second);
    assert_eq!(h.paths("is:all groceries").len(), 2);
}

#[test]
fn adding_below_a_done_item_starts_a_fresh_one() {
    let mut h = Harness::new();
    h.add("home/weekly/buy groceries");
    h.try_patch(
        "home/weekly",
        PatchItem {
            status: Some("done".into()),
            force: true,
            ..Default::default()
        },
    )
    .unwrap();
    h.add("home/weekly/clean");
    assert_eq!(h.paths(""), ["home", "home/weekly", "home/weekly/clean"]);
    assert_eq!(h.paths("is:all weekly").len(), 4);
}

#[test]
fn display_order_keeps_children_under_their_parent() {
    let mut h = Harness::new();
    h.add("home/weekly.old");
    h.add("home/weekly/a");
    h.add("home/weekly-x");
    assert_eq!(
        h.paths(""),
        [
            "home",
            "home/weekly",
            "home/weekly/a",
            "home/weekly-x",
            "home/weekly.old",
        ]
    );
}

#[test]
fn rename_and_move_rewrite_every_descendant_path() {
    let mut h = Harness::new();
    h.add("home/weekly/renovations/kitchen");
    h.add("work");
    h.try_patch(
        "home/weekly/renovations",
        PatchItem {
            path: Some("work/Flat".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        h.paths(""),
        [
            "home",
            "home/weekly",
            "work",
            "work/flat",
            "work/flat/kitchen"
        ]
    );
    h.assert_matches_rebuild();
}

#[test]
fn moving_creates_missing_parents_but_not_inside_itself() {
    let mut h = Harness::new();
    h.add("home/weekly/renovations/kitchen");
    let updated = h
        .try_patch(
            "home/weekly/renovations",
            PatchItem {
                path: Some("archive/2026/flat".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(updated.changed);
    assert_eq!(updated.created.len(), 2);
    assert_eq!(
        h.paths(""),
        [
            "archive",
            "archive/2026",
            "archive/2026/flat",
            "archive/2026/flat/kitchen",
            "home",
            "home/weekly",
        ]
    );

    let before = h.tree.clone();
    let into_self = h.try_patch(
        "archive",
        PatchItem {
            path: Some("archive/2026/new/archive".into()),
            ..Default::default()
        },
    );
    assert_eq!(into_self.unwrap_err().kind, ErrorKind::Conflict);
    assert_eq!(h.tree, before);
    h.assert_matches_rebuild();
}

#[test]
fn moves_into_own_subtree_or_onto_an_open_name_conflict() {
    let mut h = Harness::new();
    h.add("home/weekly/renovations");
    h.add("work/weekly");
    let into_self = h.try_patch(
        "home",
        PatchItem {
            path: Some("home/weekly/renovations/home".into()),
            ..Default::default()
        },
    );
    assert_eq!(into_self.unwrap_err().kind, ErrorKind::Conflict);
    let onto_name = h.try_patch(
        "work/weekly",
        PatchItem {
            path: Some("home/weekly".into()),
            ..Default::default()
        },
    );
    assert_eq!(onto_name.unwrap_err().kind, ErrorKind::Conflict);
}

#[test]
fn done_with_open_children_needs_force_and_hides_the_subtree() {
    let mut h = Harness::new();
    h.add("home/weekly/renovations/kitchen");
    let err = h.set_status("home/weekly/renovations", "done").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);

    h.try_patch(
        "home/weekly/renovations",
        PatchItem {
            status: Some("done".into()),
            force: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(h.paths(""), ["home", "home/weekly"]);
    let kitchen = h
        .tree
        .items()
        .find(|i| i.title == "kitchen")
        .unwrap()
        .clone();
    assert_eq!(h.tree.status(kitchen.status).unwrap().name, "todo");
    assert_eq!(
        h.paths("is:done"),
        ["home/weekly/renovations", "home/weekly/renovations/kitchen"]
    );
    h.assert_matches_rebuild();
}

#[test]
fn reopening_conflicts_when_the_name_was_reused_meanwhile() {
    let mut h = Harness::new();
    let first = h.add("home/weekly/buy groceries");
    h.set_status("home/weekly/buy groceries", "done").unwrap();
    h.add("home/weekly/buy groceries");
    let now = h.now();
    let err = h
        .tree
        .update(
            first,
            &PatchItem {
                status: Some("todo".into()),
                ..Default::default()
            },
            now,
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
}

#[test]
fn status_changes_stamp_times_and_noop_patches_report_no_change() {
    let mut h = Harness::new();
    let id = h.add("home/fix wifi");
    let now = h.now();
    let updated = h
        .tree
        .update(
            id,
            &PatchItem {
                status: Some("progress".into()),
                tags: Some(vec!["Net".into()]),
                ..Default::default()
            },
            now,
        )
        .unwrap();
    assert!(updated.changed);
    let item = h.tree.get(id).unwrap().item.clone();
    assert_eq!(item.started_at, Some(now));
    assert_eq!(item.updated_at, now);
    assert_eq!(item.tags, ["net"]);

    let later = h.now();
    assert!(
        !h.tree
            .update(id, &PatchItem::default(), later)
            .unwrap()
            .changed
    );
    assert_eq!(h.tree.get(id).unwrap().item.updated_at, now);
}

#[test]
fn remove_needs_recursive_for_children_and_removes_the_subtree() {
    let mut h = Harness::new();
    h.add("home/weekly/renovations/kitchen");
    h.add("home/garden");
    let weekly = h.id("home/weekly");
    assert_eq!(
        h.tree.remove(weekly, false).unwrap_err().kind,
        ErrorKind::Conflict
    );
    let removed = h.tree.remove(weekly, true).unwrap();
    assert_eq!(removed.len(), 3);
    assert_eq!(removed.last(), Some(&weekly), "subtree root comes last");
    assert_eq!(h.paths("is:all"), ["home", "home/garden"]);
    h.assert_matches_rebuild();
}

#[test]
fn ids_are_not_reused_after_a_delete() {
    let mut h = Harness::new();
    let first = h.add("a");
    h.tree.remove(first, false).unwrap();
    let second = h.add("b");
    assert!(second > first);
}

#[test]
fn list_words_operators_and_subtrees() {
    let mut h = Harness::new();
    h.add("home/weekly/buy groceries");
    let now = h.now();
    h.tree
        .add(
            &CreateItem {
                path: "home/weekly/renovations/kitchen".into(),
                tags: vec!["diy".into()],
                ..Default::default()
            },
            now,
        )
        .unwrap();
    h.add("work/weekly report");

    assert_eq!(
        h.paths("renov"),
        ["home/weekly/renovations", "home/weekly/renovations/kitchen"]
    );
    assert_eq!(h.paths("groc weekly"), ["home/weekly/buy_groceries"]);
    assert_eq!(h.paths("\"weekly report\""), ["work/weekly_report"]);
    assert_eq!(h.paths("weekly -home"), ["work/weekly_report"]);
    assert_eq!(h.paths("+diy"), ["home/weekly/renovations/kitchen"]);
    assert_eq!(
        h.paths("tag:diy kitchen"),
        ["home/weekly/renovations/kitchen"]
    );
    assert_eq!(h.paths("status:progress"), Vec::<String>::new());
}

#[test]
fn list_reports_unknown_status() {
    let h = Harness::new();
    let unknown = h.tree.list(&Query::parse("status:blocked").unwrap());
    assert_eq!(unknown.unwrap_err().kind, ErrorKind::Invalid);
}

#[test]
fn statuses_in_use_or_last_of_category_are_protected() {
    let mut h = Harness::new();
    h.add("home");
    h.tree
        .add_status(&CreateStatus {
            name: "Blocked".into(),
            category: Category::Active,
            label: None,
        })
        .unwrap();
    assert_eq!(h.tree.status_by_name("blocked").unwrap().position, 4);

    assert_eq!(
        h.tree.remove_status("todo").unwrap_err().kind,
        ErrorKind::Conflict,
        "in use"
    );
    assert_eq!(
        h.tree.remove_status("done").unwrap_err().kind,
        ErrorKind::Conflict,
        "last done status"
    );
    let recategorize = h.tree.update_status(
        "todo",
        &PatchStatus {
            category: Some(Category::Active),
            ..Default::default()
        },
    );
    assert_eq!(recategorize.unwrap_err().kind, ErrorKind::Conflict);

    h.tree.remove_status("blocked").unwrap();
    assert!(h.tree.status_by_name("blocked").is_err());
}

#[test]
fn rows_with_a_parent_cycle_are_rejected() {
    let mut h = Harness::new();
    let a = h.add("a");
    let b = h.add("a/b");
    let mut items: Vec<_> = h.tree.items().cloned().collect();
    for item in &mut items {
        if item.id == a {
            item.parent = Some(b);
        }
    }
    let err = Tree::new(seed_statuses(), items, 1).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Internal);
}

#[test]
fn incremental_updates_match_a_full_rebuild() {
    let mut h = Harness::new();
    let mut rng = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move |n: usize| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng % n as u64) as usize
    };
    let words = [
        "home", "work", "weekly", "kitchen", "garden", "taxes", "car",
    ];
    let statuses = ["todo", "progress", "done"];
    for step in 0..1_000 {
        let ids: Vec<ItemId> = h.tree.items().map(|i| i.id).collect();
        let pick = |n: usize| ids[n];
        match next(10) {
            0..=3 => {
                let depth = 1 + next(3);
                let path: Vec<&str> = (0..depth).map(|_| words[next(words.len())]).collect();
                let _ = h.try_add(&path.join("/"));
            }
            4 | 5 if !ids.is_empty() => {
                let id = pick(next(ids.len()));
                let now = h.now();
                let patch = PatchItem {
                    status: Some(statuses[next(3)].into()),
                    force: next(2) == 0,
                    ..Default::default()
                };
                let _ = h.tree.update(id, &patch, now);
            }
            6 | 7 if !ids.is_empty() => {
                let id = pick(next(ids.len()));
                let now = h.now();
                let title = if next(2) == 0 {
                    words[next(words.len())].to_string()
                } else {
                    h.tree.get(id).unwrap().item.title.clone()
                };
                let path = if next(4) == 0 {
                    title
                } else {
                    let parent = pick(next(ids.len()));
                    format!("{}/{}", h.tree.get(parent).unwrap().path, title)
                };
                let patch = PatchItem {
                    path: Some(path),
                    ..Default::default()
                };
                let _ = h.tree.update(id, &patch, now);
            }
            8 if !ids.is_empty() => {
                let id = pick(next(ids.len()));
                let _ = h.tree.remove(id, next(4) == 0);
            }
            _ => {}
        }
        if step % 50 == 0 {
            h.assert_matches_rebuild();
        }
    }
    assert!(h.tree.len() > 20, "the sequence should build a real tree");
    h.assert_matches_rebuild();
}
