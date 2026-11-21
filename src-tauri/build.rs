fn main() {
    tauri_build::build();
    #[cfg(windows)]
    if std::env::var("PROFILE").as_deref() == Ok("debug") {
        embed_test_manifest();
    }
}

// The lib test binary links the Tauri GUI stack (comctl32 v6 TaskDialogIndirect),
// but cargo's test target gets no app manifest, so Windows loads comctl32 v5 and
// the test exe fails to start with STATUS_ENTRYPOINT_NOT_FOUND. Embed the v6
// common-controls dependency into test binaries so `cargo test` can run.
#[cfg(windows)]
fn embed_test_manifest() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"))
        .join("notcode-test-manifest.xml");
    std::fs::write(&out, "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<assembly xmlns=\"urn:schemas-microsoft-com:asm.v1\" manifestVersion=\"1.0\">\n\
  <dependency><dependentAssembly>\n\
    <assemblyIdentity type=\"win32\" name=\"Microsoft.Windows.Common-Controls\" version=\"6.0.0.0\" processorArchitecture=\"*\" publicKeyToken=\"6595b64144ccf1df\" language=\"*\"/>\n\
  </dependentAssembly></dependency>\n\
</assembly>\n").expect("write test manifest");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", out.display());
}
