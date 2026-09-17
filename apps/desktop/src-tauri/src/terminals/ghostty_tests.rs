use super::ghostty_applescript;

#[test]
fn launches_shell_script_in_a_new_window() {
    assert_eq!(
        ghostty_applescript("cd '/tmp/project' && codex --resume 'session id'"),
        "tell application \"Ghostty\"\nactivate\nset newWindow to new window\nset newTerminal to focused terminal of selected tab of newWindow\ninput text (\"cd '/tmp/project' && codex --resume 'session id'\" & return) to newTerminal\nend tell"
    );
}
