fn main() {
    println!("cargo:rerun-if-changed=assets/Info.plist");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let plist = std::path::Path::new(&manifest).join("assets/Info.plist");
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("macos") {
        println!(
            "cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            plist.display()
        );
    }
}
