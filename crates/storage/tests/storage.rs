use jiff::Timestamp;
use tack_core::ErrorKind;
use tack_core::api::{CreateItem, PatchItem};
use tack_core::model::ItemId;
use tack_core::path::Target;
use tack_core::tree::{Tree, Written};
use tack_storage::Storage;

fn at(second: i64) -> Timestamp {
    Timestamp::from_second(second).unwrap()
}

fn create(path: &str) -> CreateItem {
    CreateItem {
        path: path.into(),
        ..Default::default()
    }
}

fn id_of(tree: &Tree, path: &str) -> ItemId {
    tree.find(&path.parse::<Target>().unwrap()).unwrap().item.id
}

fn save(storage: &mut Storage, tree: &Tree, written: &Written) {
    let items =
        |ids: &[ItemId]| -> Vec<_> { ids.iter().map(|id| &tree.get(*id).unwrap().item).collect() };
    storage
        .save(&items(&written.created), &items(&written.updated))
        .unwrap();
}

fn add(storage: &mut Storage, tree: &mut Tree, req: &CreateItem, now: Timestamp) {
    let written = tree.add(req, now).unwrap();
    save(storage, tree, &written);
}

fn update(storage: &mut Storage, tree: &mut Tree, id: ItemId, patch: &PatchItem, now: Timestamp) {
    let written = tree.update(id, patch, now).unwrap();
    save(storage, tree, &written);
}

#[test]
fn fresh_database_is_migrated_and_seeded() {
    let storage = Storage::open_in_memory().unwrap();
    assert_eq!(storage.schema_version().unwrap(), 1);
    let tree = storage.load().unwrap();
    let names: Vec<_> = tree.statuses().iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["todo", "doing", "done"]);
    assert!(tree.is_empty());
}

#[test]
fn live_tree_equals_a_fresh_load_after_every_kind_of_write() {
    let mut storage = Storage::open_in_memory().unwrap();
    let mut tree = storage.load().unwrap();

    add(
        &mut storage,
        &mut tree,
        &CreateItem {
            path: "home/weekly/buy groceries".into(),
            tags: vec!["food".into(), "shop".into()],
            body: Some("Milk, eggs.".into()),
            ..Default::default()
        },
        at(10),
    );
    add(
        &mut storage,
        &mut tree,
        &create("home/weekly/renovations/kitchen"),
        at(11),
    );
    add(&mut storage, &mut tree, &create("work"), at(12));

    let renovations = id_of(&tree, "home/weekly/renovations");
    update(
        &mut storage,
        &mut tree,
        renovations,
        &PatchItem {
            path: Some("work/archive/flat".into()),
            status: Some("doing".into()),
            ..Default::default()
        },
        at(13),
    );
    let groceries = id_of(&tree, "home/weekly/buy groceries");
    update(
        &mut storage,
        &mut tree,
        groceries,
        &PatchItem {
            tags: Some(vec!["food".into()]),
            status: Some("done".into()),
            ..Default::default()
        },
        at(14),
    );

    let work = id_of(&tree, "work");
    update(
        &mut storage,
        &mut tree,
        work,
        &PatchItem {
            archived: Some(true),
            due: Some(Some(jiff::civil::date(2026, 12, 24))),
            tags: Some(vec!["office".into(), "food".into()]),
            ..Default::default()
        },
        at(15),
    );
    let colored = tree.set_tag_color("office", Some("teal")).unwrap();
    storage.set_tag_color(&colored).unwrap();
    let deleted = tree.remove_tag("shop").unwrap();
    storage.delete_tag(&deleted).unwrap();
    add(&mut storage, &mut tree, &create("work/new"), at(16));
    assert_eq!(tree.get(work).unwrap().item.archived_at, None);

    let home = id_of(&tree, "home");
    let removed = tree.remove(home, true).unwrap();
    storage.delete_items(&removed).unwrap();

    assert_eq!(tree, storage.load().unwrap());
    let paths: Vec<_> = tree.iter().map(|n| n.path.to_string()).collect();
    assert_eq!(
        paths,
        [
            "/work",
            "/work/archive",
            "/work/archive/flat",
            "/work/archive/flat/kitchen",
            "/work/new"
        ]
    );
}

#[test]
fn a_stale_tree_is_a_conflict_and_writes_nothing() {
    let mut storage = Storage::open_in_memory().unwrap();
    let mut live = storage.load().unwrap();
    let mut stale = storage.load().unwrap();

    add(&mut storage, &mut live, &create("home/x"), at(10));

    let written = stale.add(&create("home/y"), at(11)).unwrap();
    let items: Vec<_> = written
        .created
        .iter()
        .map(|id| &stale.get(*id).unwrap().item)
        .collect();
    let err = storage.save(&items, &[]).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
    assert_eq!(storage.load().unwrap(), live);
}

#[test]
fn file_database_reopens_without_reusing_ids_and_rejects_newer_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/dir/tack.db");
    let deleted = {
        let mut storage = Storage::open(&path).unwrap();
        let mut tree = storage.load().unwrap();
        add(&mut storage, &mut tree, &create("home"), at(10));
        add(&mut storage, &mut tree, &create("work"), at(11));
        let work = id_of(&tree, "work");
        storage
            .delete_items(&tree.remove(work, false).unwrap())
            .unwrap();
        work
    };

    let mut storage = Storage::open(&path).unwrap();
    let mut tree = storage.load().unwrap();
    assert_eq!(tree.len(), 1);
    add(&mut storage, &mut tree, &create("garden"), at(12));
    assert!(id_of(&tree, "garden") > deleted);
    drop(storage);

    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    let err = Storage::open(&path)
        .err()
        .expect("newer schema must be rejected");
    assert!(err.message.contains("newer"), "{}", err.message);
}

#[test]
fn allowed_clients_round_trip_and_deny() {
    let mut storage = Storage::open_in_memory().unwrap();
    assert!(storage.allowed_clients().unwrap().is_empty());
    let laptop: std::net::IpAddr = "192.168.1.20".parse().unwrap();
    storage.allow_client(laptop).unwrap();
    storage.allow_client(laptop).unwrap();
    assert_eq!(storage.allowed_clients().unwrap(), [laptop]);
    assert!(storage.deny_client(laptop).unwrap());
    assert!(!storage.deny_client(laptop).unwrap());
    assert!(storage.allowed_clients().unwrap().is_empty());
}
