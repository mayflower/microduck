fn main() {
    println!("cargo:rerun-if-changed=native");
    println!("cargo:rerun-if-changed=vendor");
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .opt_level(2)
        .warnings(false)
        .file("vendor/rubberband/single/RubberBandSingle.cpp")
        .file("native/process.cpp")
        .compile("duck_voice_color");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=Accelerate");
    }
}
