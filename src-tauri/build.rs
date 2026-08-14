fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=windows-test-manifest.rc");
        println!("cargo:rerun-if-changed=windows-app-manifest.xml");
        embed_resource::compile_for_tests("windows-test-manifest.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }

    tauri_build::build();
}
