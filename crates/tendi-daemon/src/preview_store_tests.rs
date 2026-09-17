use super::*;

#[test]
fn consuming_one_preview_preserves_other_plans() {
    let mut previews = PreviewStore::default();
    previews.insert("a".into(), 10).unwrap();
    previews.insert("b".into(), 20).unwrap();
    assert_eq!(previews.remove("a"), Some(10));
    assert_eq!(previews.remove("a"), None);
    assert_eq!(previews.get("b"), Some(&20));
}

#[test]
fn full_store_rejects_new_previews_without_evicting_existing_plans() {
    let mut previews = PreviewStore {
        capacity: 1,
        ..Default::default()
    };
    previews.insert("a".into(), 10).unwrap();
    assert!(previews.insert("b".into(), 20).is_err());
    assert_eq!(previews.get("a"), Some(&10));
}

#[test]
fn expired_preview_cannot_be_read_or_applied() {
    let mut previews = PreviewStore::<()>::default();
    previews
        .entries
        .insert("expired".into(), (Instant::now() - previews.ttl, ()));
    assert_eq!(previews.get("expired"), None);
    assert_eq!(previews.remove("expired"), None);
}
