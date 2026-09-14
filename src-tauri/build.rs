fn main() {
    tauri_build::build();

    // tauri-build manifests bin targets only; test executables link the same
    // GUI stack (tao/muda → comctl32 v6) and would fail to load with
    // STATUS_ENTRYPOINT_NOT_FOUND without the Common-Controls v6 dependency.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/tests.manifest");
        println!("cargo:warning=emitting test manifest link args");
        println!("cargo::rustc-link-arg-tests=/MANIFEST:EMBED");

        println!("cargo:rustc-link-arg-tests=/MANIFESTUAC:NO");
        println!("cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}", manifest);
    }
}
