fn main() {
    // Android devices with 16 KB pages refuse a library aligned to 4 KB.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-z,max-page-size=16384");
    }
}
