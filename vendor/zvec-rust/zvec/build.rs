use std::env;

fn main() {
    // The jieba dict auto-discovery uses dladdr() to locate the loaded
    // libzvec_c_api at runtime. On Linux dladdr lives in libdl (merged into
    // libc on glibc >= 2.34, but -ldl remains valid there and on musl).
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-lib=dylib=dl");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
