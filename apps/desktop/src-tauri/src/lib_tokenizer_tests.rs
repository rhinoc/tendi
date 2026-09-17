use super::tokenizer_count_blocking;
use tendi_core::generated::runtime_contract::TokenizerCountRequest;

#[test]
fn tokenizer_count_uses_o200k_base() {
    let response = tokenizer_count_blocking(TokenizerCountRequest {
        texts: vec![
            "hello world".to_string(),
            "中文 tokenization".to_string(),
            "```rust\nfn main() {}\n```".to_string(),
        ],
    })
    .expect("tokenizer count should succeed");
    assert_eq!(response.counts, vec![2, 3, 8]);
}
