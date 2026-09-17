use super::*;

#[test]
fn encodes_marketplace_query_parameters() {
    assert_eq!(
        encode_query("react native/中文"),
        "react%20native%2F%E4%B8%AD%E6%96%87"
    );
}
