fn main() {
    println!("cargo:rerun-if-changed=src/session_ownership/macos.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/session_ownership/macos.c")
            .compile("tendi_process_info");
    }
}
