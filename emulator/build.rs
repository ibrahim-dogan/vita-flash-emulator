fn main() {
    #[cfg(target_os = "horizon")]
    {
        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:rustc-link-lib=dylib=nx");
    }
}
