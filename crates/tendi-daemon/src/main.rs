use std::{env, io::Write, net::TcpListener, path::PathBuf};

fn main() -> std::io::Result<()> {
    let logger = tendi_core::logging::init("tendi-daemon").map_err(std::io::Error::other)?;
    let mut port = 5189_u16;
    let mut workspace = env::var_os("TENDI_CWD")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?);
    let mut token = env::var("TENDI_DAEMON_TOKEN").ok();
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--port" => {
                index += 1;
                port = args
                    .get(index)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(port);
            }
            "--workspace" => {
                index += 1;
                if let Some(value) = args.get(index) {
                    workspace = PathBuf::from(value);
                }
            }
            "--token" => {
                index += 1;
                token = args.get(index).cloned();
            }
            value => logger.warn(
                "ignoring unknown daemon argument",
                serde_json::json!({ "argument": value }),
            ),
        }
        index += 1;
    }
    let daemon = tendi_daemon::Daemon::new(workspace.clone());
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    logger.info(
        "daemon listening",
        serde_json::json!({
            "port": port,
            "workspace": workspace,
            "log_path": tendi_core::logging::Logger::path().ok(),
        }),
    );
    let result = tendi_daemon::run_http(daemon, listener, token);
    if let Err(error) = &result {
        logger.error(
            "daemon stopped with error",
            serde_json::json!({ "error": error.to_string() }),
        );
        let _ = writeln!(std::io::stderr(), "tendi daemon stopped: {error}");
    }
    result
}
