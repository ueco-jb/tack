use jiff::Timestamp;
use tack_core::ErrorKind;
use tack_core::api::{CreateItem, CreateStatus, PatchItem, PatchStatus};
use tack_core::model::{Category, ItemId, Status, StatusId, TAG_COLORS};
use tack_core::path::Target;
use tack_core::query::Query;
use tack_core::tree::{Tree, Written};

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
        status(2, "doing", Category::Active, 2),
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
            tree: Tree::new(seed_statuses(), vec![], vec![], 1).unwrap(),
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
        Ok(*created.created.last().unwrap())
    }

    fn add(&mut self, title: &str) -> ItemId {
        self.try_add(title).unwrap()
    }

    fn try_patch(&mut self, target: &str, patch: PatchItem) -> tack_core::Result<Written> {
        let id = self.id(target);
        let now = self.now();
        self.tree.update(id, &patch, now)
    }

    fn set_status(&mut self, target: &str, status: &str) -> tack_core::Result<Written> {
        self.try_patch(
            target,
            PatchItem {
                status: Some(status.into()),
                ..Default::default()
            },
        )
    }

    fn archive(&mut self, target: &str, archived: bool) -> tack_core::Result<Written> {
        self.try_patch(
            target,
            PatchItem {
                archived: Some(archived),
                ..Default::default()
            },
        )
    }

    fn path_of(&self, id: ItemId) -> String {
        self.tree.get(id).unwrap().path.to_string()
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
            self.tree.tags(),
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
            "/home",
            "/home/weekly",
            "/home/weekly/buy_groceries",
            "/home/weekly/renovations",
            "/home/weekly/renovations/kitchen",
        ]
    );
    let again = h.add("home/weekly/buy_groceries");
    assert_eq!(h.path_of(again), "/home/weekly/buy_groceries-2");
}

#[test]
fn a_rejected_add_changes_nothing() {
    let mut h = Harness::new();
    h.add("home/weekly/buy groceries");
    let before = h.tree.clone();
    let now = h.now();
    let err = h
        .tree
        .add(
            &CreateItem {
                path: "home/garden/beds".into(),
                status: Some("nope".into()),
                ..Default::default()
            },
            now,
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Invalid);
    assert_eq!(h.tree, before);
}

#[test]
fn adding_or_moving_under_an_archived_item_unarchives_its_chain() {
    let mut h = Harness::new();
    h.add("home/weekly/clean");
    h.add("work/report");
    h.archive("home", true).unwrap();
    h.archive("home/weekly", true).unwrap();

    let now = h.now();
    let written = h
        .tree
        .add(
            &CreateItem {
                path: "home/weekly/laundry".into(),
                ..Default::default()
            },
            now,
        )
        .unwrap();
    let mut unarchived = written.updated.clone();
    unarchived.sort();
    let mut expected = vec![h.id("home"), h.id("home/weekly")];
    expected.sort();
    assert_eq!(unarchived, expected);
    assert!(h.paths("is:archived").is_empty());
    assert_eq!(h.paths("").len(), 6);

    h.archive("home", true).unwrap();
    let moved = h
        .try_patch(
            "work/report",
            PatchItem {
                path: Some("home/weekly/report".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(moved.updated, [h.id("home"), h.id("home/weekly/report")]);
    assert!(h.paths("is:archived").is_empty());
    h.assert_matches_rebuild();
}

#[test]
fn repeated_names_get_the_next_free_number_whatever_their_state() {
    let mut h = Harness::new();
    h.add("home/weekly/cleaning");
    h.set_status("home/weekly/cleaning", "done").unwrap();
    let second = h.add("home/weekly/cleaning");
    h.archive("home/weekly/cleaning", true).unwrap();
    let third = h.add("home/weekly/cleaning");
    assert_eq!(h.path_of(second), "/home/weekly/cleaning-2");
    assert_eq!(h.path_of(third), "/home/weekly/cleaning-3");
    assert_eq!(
        h.paths(""),
        [
            "/home",
            "/home/weekly",
            "/home/weekly/cleaning-2",
            "/home/weekly/cleaning-3"
        ]
    );
    assert_eq!(h.paths("is:archived"), ["/home/weekly/cleaning"]);

    h.tree.remove(second, false).unwrap();
    let fourth = h.add("home/weekly/cleaning");
    assert_eq!(h.path_of(fourth), "/home/weekly/cleaning-2");
}

#[test]
fn done_items_stay_listed_and_take_new_children() {
    let mut h = Harness::new();
    h.add("home/weekly/buy groceries");
    h.set_status("home/weekly/buy groceries", "done").unwrap();
    h.add("home/weekly/buy groceries/milk");
    assert_eq!(
        h.paths(""),
        [
            "/home",
            "/home/weekly",
            "/home/weekly/buy_groceries",
            "/home/weekly/buy_groceries/milk"
        ]
    );
    assert_eq!(h.paths("is:done"), ["/home/weekly/buy_groceries"]);
    assert_eq!(h.paths("is:open groc"), ["/home/weekly/buy_groceries/milk"]);
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
            "/home",
            "/home/weekly",
            "/home/weekly/a",
            "/home/weekly-x",
            "/home/weekly.old",
        ]
    );
}

#[test]
fn rename_and_move_rewrite_every_descendant_path() {
    let mut h = Harness::new();
    h.add("home/weekly/renovations/kitchen");
    h.add("work");
    h.try_patch(
        "/home/weekly/renovations",
        PatchItem {
            path: Some("work/Flat".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        h.paths(""),
        [
            "/home",
            "/home/weekly",
            "/work",
            "/work/flat",
            "/work/flat/kitchen"
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
            "/home/weekly/renovations",
            PatchItem {
                path: Some("archive/2026/flat".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!updated.updated.is_empty());
    assert_eq!(updated.created.len(), 2);
    assert_eq!(
        h.paths(""),
        [
            "/archive",
            "/archive/2026",
            "/archive/2026/flat",
            "/archive/2026/flat/kitchen",
            "/home",
            "/home/weekly",
        ]
    );

    let before = h.tree.clone();
    let into_self = h.try_patch(
        "/archive",
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
fn moving_into_own_subtree_conflicts_and_taken_names_get_numbered() {
    let mut h = Harness::new();
    h.add("home/weekly/renovations");
    h.add("work/weekly");
    let into_self = h.try_patch(
        "/home",
        PatchItem {
            path: Some("home/weekly/renovations/home".into()),
            ..Default::default()
        },
    );
    assert_eq!(into_self.unwrap_err().kind, ErrorKind::Conflict);
    let onto_name = h
        .try_patch(
            "/work/weekly",
            PatchItem {
                path: Some("home/weekly".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!onto_name.updated.is_empty());
    assert!(h.paths("").contains(&"/home/weekly-2".to_string()));
    h.assert_matches_rebuild();
}

#[test]
fn done_with_open_children_needs_force_and_archive_hides_the_subtree() {
    let mut h = Harness::new();
    h.add("home/weekly/renovations/kitchen");
    let err = h.set_status("home/weekly/renovations", "done").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);

    h.try_patch(
        "/home/weekly/renovations",
        PatchItem {
            status: Some("done".into()),
            force: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(h.paths("").len(), 4, "done items stay listed");

    h.archive("home/weekly/renovations", true).unwrap();
    assert_eq!(h.paths(""), ["/home", "/home/weekly"]);
    assert_eq!(
        h.paths("is:archived"),
        [
            "/home/weekly/renovations",
            "/home/weekly/renovations/kitchen"
        ]
    );
    let kitchen = h.id("home/weekly/renovations/kitchen");
    assert_eq!(h.tree.get(kitchen).unwrap().item.archived_at, None);
    h.assert_matches_rebuild();

    h.archive("home/weekly/renovations", false).unwrap();
    assert_eq!(h.paths("").len(), 4);
    assert!(h.paths("is:archived").is_empty());
    h.assert_matches_rebuild();
}

#[test]
fn unarchiving_a_child_unarchives_its_whole_chain() {
    let mut h = Harness::new();
    h.add("home/weekly/clean");
    h.add("home/garden");
    h.archive("home", true).unwrap();
    h.archive("home/weekly", true).unwrap();
    assert!(h.paths("").is_empty());

    let written = h.archive("home/weekly/clean", false).unwrap();
    let mut updated = written.updated;
    updated.sort();
    let mut expected = vec![h.id("home"), h.id("home/weekly"), h.id("home/weekly/clean")];
    expected.sort();
    assert_eq!(updated, expected);
    assert!(h.paths("is:archived").is_empty());
    assert_eq!(h.paths("").len(), 4);
    h.assert_matches_rebuild();
}

#[test]
fn archived_children_do_not_block_marking_the_parent_done() {
    let mut h = Harness::new();
    h.add("home/weekly/clean");
    h.archive("home/weekly/clean", true).unwrap();
    assert!(
        !h.set_status("home/weekly", "done")
            .unwrap()
            .updated
            .is_empty()
    );
}

#[test]
fn tags_keep_their_color_until_deleted() {
    let mut h = Harness::new();
    let now = h.now();
    let id = h
        .tree
        .add(
            &CreateItem {
                path: "home/fix wifi".into(),
                tags: vec!["net".into(), "/home".into()],
                ..Default::default()
            },
            now,
        )
        .unwrap()
        .created[1];
    let fresh = h.tree.tag("net").unwrap().color.unwrap();
    assert!(
        TAG_COLORS.contains(&fresh.as_str()),
        "new tags get a palette color"
    );
    h.tree.set_tag_color("NET", Some("blue")).unwrap();
    h.try_patch(
        "/home/fix wifi",
        PatchItem {
            tags: Some(vec![]),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        h.tree.tag("net").unwrap().color.as_deref(),
        Some("blue"),
        "an unused tag keeps its color"
    );
    assert_eq!(
        h.tree.set_tag_color("nope", Some("red")).unwrap_err().kind,
        ErrorKind::NotFound
    );

    h.try_patch(
        "/home/fix wifi",
        PatchItem {
            tags: Some(vec!["/home".into(), "net".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    h.tree.remove_tag("net").unwrap();
    assert_eq!(h.tree.get(id).unwrap().item.tags, ["/home"]);
    assert_eq!(
        h.tree
            .tags()
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["/home"]
    );
    h.assert_matches_rebuild();
}

#[test]
fn due_dates_are_optional_and_can_be_cleared() {
    let mut h = Harness::new();
    let now = h.now();
    let due = jiff::civil::date(2026, 11, 20);
    let id = *h
        .tree
        .add(
            &CreateItem {
                path: "home/taxes".into(),
                due: Some(due),
                ..Default::default()
            },
            now,
        )
        .unwrap()
        .created
        .last()
        .unwrap();
    assert_eq!(h.tree.view(id).unwrap().due, Some(due));
    assert_eq!(
        h.tree.view(h.id("home")).unwrap().due,
        None,
        "parents get no due date"
    );

    h.try_patch(
        "home/taxes",
        PatchItem {
            body: Some("forms".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(h.tree.view(id).unwrap().due, Some(due), "absent keeps it");
    h.try_patch(
        "home/taxes",
        PatchItem {
            due: Some(None),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(h.tree.view(id).unwrap().due, None, "null clears it");
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
                status: Some("doing".into()),
                tags: Some(vec!["Net".into()]),
                ..Default::default()
            },
            now,
        )
        .unwrap();
    assert!(!updated.updated.is_empty());
    let item = h.tree.get(id).unwrap().item.clone();
    assert_eq!(item.started_at, Some(now));
    assert_eq!(item.updated_at, now);
    assert_eq!(item.tags, ["net"]);

    let later = h.now();
    assert!(
        h.tree
            .update(id, &PatchItem::default(), later)
            .unwrap()
            .updated
            .is_empty()
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
    assert_eq!(h.paths("is:all"), ["/home", "/home/garden"]);
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
        [
            "/home/weekly/renovations",
            "/home/weekly/renovations/kitchen"
        ]
    );
    assert_eq!(h.paths("groc weekly"), ["/home/weekly/buy_groceries"]);
    assert_eq!(h.paths("\"weekly report\""), ["/work/weekly_report"]);
    assert_eq!(h.paths("weekly -home"), ["/work/weekly_report"]);
    assert_eq!(
        h.paths("/home/weekly/renovations"),
        [
            "/home/weekly/renovations",
            "/home/weekly/renovations/kitchen"
        ]
    );
    assert!(
        h.paths("/weekly").is_empty(),
        "a leading slash anchors at the root"
    );
    assert_eq!(h.paths("weekly -/home"), ["/work/weekly_report"]);
    assert_eq!(h.paths("+diy"), ["/home/weekly/renovations/kitchen"]);
    assert_eq!(
        h.paths("tag:diy kitchen"),
        ["/home/weekly/renovations/kitchen"]
    );
    assert_eq!(h.paths("status:doing"), Vec::<String>::new());
}

#[test]
fn typed_status_names_and_labels_filter_by_status() {
    let mut h = Harness::new();
    h.add("home/laundry");
    h.add("home/dishes");
    h.add("work/todo app");
    h.set_status("home/dishes", "doing").unwrap();
    h.tree
        .add_status(&CreateStatus {
            name: "review".into(),
            category: Category::Active,
            label: Some("In review".into()),
        })
        .unwrap();
    h.set_status("work/todo app", "review").unwrap();

    assert_eq!(h.paths("doing"), ["/home/dishes"]);
    assert_eq!(h.paths("todo home"), ["/home", "/home/laundry"]);
    assert_eq!(
        h.paths("todo doing /home"),
        ["/home", "/home/dishes", "/home/laundry"]
    );
    assert_eq!(h.paths("In Review"), ["/work/todo_app"]);
    assert_eq!(
        h.paths("todo_app"),
        ["/work/todo_app"],
        "a longer word still searches paths"
    );
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
    let err = Tree::new(seed_statuses(), items, vec![], 1).unwrap_err();
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
        "/home", "/work", "weekly", "kitchen", "garden", "taxes", "car",
    ];
    let statuses = ["todo", "doing", "done"];
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
            9 if !ids.is_empty() => {
                let id = pick(next(ids.len()));
                let now = h.now();
                let patch = PatchItem {
                    archived: Some(next(2) == 0),
                    ..Default::default()
                };
                let _ = h.tree.update(id, &patch, now);
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
