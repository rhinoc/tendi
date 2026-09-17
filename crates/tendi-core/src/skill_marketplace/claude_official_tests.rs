use super::*;

#[test]
fn converts_relative_plugin_source() {
    let source = PluginSource::Path("./plugins/example".to_string());
    assert_eq!(
        plugin_source(&source).as_deref(),
        Some("https://github.com/anthropics/claude-plugins-official/tree/main/plugins/example")
    );
}
