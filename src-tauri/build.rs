fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=windows-test-manifest.rc");
        println!("cargo:rerun-if-changed=windows-app-manifest.xml");
        embed_resource::compile_for_everything("windows-test-manifest.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();

        let attributes = tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        tauri_build::try_build(attributes).expect("failed to run Tauri build script");
        return;
    }

    tauri_build::build();
}
