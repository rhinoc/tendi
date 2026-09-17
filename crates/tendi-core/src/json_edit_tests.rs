use serde_json::json;

use super::patch_json_text;

#[test]
fn patches_nested_json_scalar_without_reformatting_siblings() {
    let source =
        "{\r\n  \"z\": 1,\r\n  \"nested\": { \"enabled\": false, \"name\": \"keep\" }\r\n}\r\n";
    let before = json!({"z": 1, "nested": {"enabled": false, "name": "keep"}});
    let after = json!({"z": 1, "nested": {"enabled": true, "name": "keep"}});

    assert_eq!(
        patch_json_text(source, &before, &after).unwrap(),
        "{\r\n  \"z\": 1,\r\n  \"nested\": { \"enabled\": true, \"name\": \"keep\" }\r\n}\r\n"
    );
}

#[test]
fn patches_json_array_insertion_without_reformatting_siblings() {
    let source = "{\n  \"keep\": true,\n  \"items\": [\n    {\"name\": \"one\"}\n  ]\n}\n";
    let before = json!({"keep": true, "items": [{"name": "one"}]});
    let after = json!({
        "keep": true,
        "items": [{"name": "one"}, {"name": "two"}]
    });

    assert_eq!(
        patch_json_text(source, &before, &after).unwrap(),
        "{\n  \"keep\": true,\n  \"items\": [\n    {\"name\": \"one\"},\n    {\"name\":\"two\"}\n  ]\n}\n"
    );
}

#[test]
fn patches_multiple_object_member_removals_without_overlapping_edits() {
    let source = "{\n  \"keep\": true,\n  \"remove_before_last\": 1,\n  \"remove_last\": 2\n}\n";
    let before = json!({
        "keep": true,
        "remove_before_last": 1,
        "remove_last": 2
    });
    let after = json!({"keep": true});

    assert_eq!(
        patch_json_text(source, &before, &after).unwrap(),
        "{\n  \"keep\": true\n}\n"
    );
}

#[test]
fn patches_non_contiguous_object_member_removals() {
    let source = "{\n  \"remove_first\": 1,\n  \"keep\": 2,\n  \"remove_last\": 3\n}\n";
    let before = json!({
        "remove_first": 1,
        "keep": 2,
        "remove_last": 3
    });
    let after = json!({"keep": 2});

    assert_eq!(
        patch_json_text(source, &before, &after).unwrap(),
        "{\n  \"keep\": 2\n}\n"
    );
}
