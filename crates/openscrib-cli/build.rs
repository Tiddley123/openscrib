//! Windows file properties (Properties -> Details): company, copyright,
//! product and description, plus the app icon.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set("ProductName", "OpenSCRIB")
        .set("FileDescription", "OpenSCRIB command-line recorder")
        .set("CompanyName", "Rovamp Services Pty Ltd")
        .set("LegalCopyright", "Copyright © 2026 Rovamp Services Pty Ltd")
        .set_icon("../../app/src-tauri/icons/icon.ico");
    if let Err(e) = res.compile() {
        // Missing resource compiler: build anyway, just without the details.
        println!("cargo:warning=could not embed Windows file properties: {e}");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
