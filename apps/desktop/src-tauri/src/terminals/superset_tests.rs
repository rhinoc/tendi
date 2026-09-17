use super::{organization_ids, workspace_id_for_path};
use serde_json::json;
use std::path::Path;

#[test]
fn finds_workspace_by_exact_worktree_path() {
    let cwd = std::env::current_dir().unwrap();
    let value = json!([
        { "id": "other", "worktreePath": "/tmp/other" },
        { "id": "current", "worktreePath": cwd },
    ]);
    assert_eq!(
        workspace_id_for_path(&value, &cwd),
        Some("current".to_string())
    );
}

#[test]
fn accepts_wrapped_workspace_lists_and_snake_case_paths() {
    let cwd = std::env::current_dir().unwrap();
    let value = json!({
        "workspaces": [{ "id": "current", "worktree_path": cwd }]
    });
    assert_eq!(
        workspace_id_for_path(&value, Path::new(&cwd)),
        Some("current".to_string())
    );
}

#[test]
fn does_not_match_a_different_worktree() {
    let value = json!([{ "id": "other", "worktreePath": "/tmp/other" }]);
    assert_eq!(
        workspace_id_for_path(&value, Path::new("/tmp/current")),
        None
    );
}

#[test]
fn reads_all_organization_ids() {
    let value = json!([
        { "id": "first", "name": "First" },
        { "id": "second", "name": "Second" }
    ]);
    assert_eq!(organization_ids(&value).unwrap(), vec!["first", "second"]);
}

#[test]
fn accepts_wrapped_organization_lists() {
    let value = json!({
        "organizations": [{ "id": "first" }, { "id": "" }, { "name": "missing" }]
    });
    assert_eq!(organization_ids(&value).unwrap(), vec!["first"]);
}
