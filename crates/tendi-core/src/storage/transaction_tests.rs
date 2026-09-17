use super::*;

#[test]
fn expired_admission_rolls_back_without_running_callback() {
    let mut connection = Connection::open_in_memory().unwrap();
    let mut called = false;
    let result = write(
        &mut connection,
        Path::new(":memory:"),
        "expired-admission",
        super::super::database_writer::WritePriority::Interactive,
        Duration::ZERO,
        Duration::ZERO,
        |_| {
            called = true;
            Ok(())
        },
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("admission expired")
    );
    assert!(!called);
    assert!(connection.is_autocommit());
}
