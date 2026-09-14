//! Regenerates `src/lib/git/bindings.ts` — commit the artifact after
//! changing Rust commands/events/DTOs (ADR-009).

use ibexgit_lib::specta_builder;

#[test]
fn export_ts_bindings() {
    // MockRuntime keeps the wry/comctl32 GUI stack out of the link.
    specta_builder::<tauri::test::MockRuntime>()
        .export(
            specta_typescript::Typescript::default(),
            "../src/lib/git/bindings.ts",
        )
        .expect("failed to export typescript bindings");
}
