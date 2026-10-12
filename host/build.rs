// FreeForGemPear — see LICENSE
// Links libpearlgpu.so (dir from $PEARLGPU_DIR, default ../gpu next to this crate). Runtime search path: the binary's own
// directory first ($ORIGIN, release tarballs ship the library next to the binary), then the build directory.
fn main() {
    let dir = std::env::var("PEARLGPU_DIR")
        .unwrap_or_else(|_| format!("{}/../gpu", std::env::var("CARGO_MANIFEST_DIR").unwrap()));
    println!("cargo:rerun-if-env-changed=PEARLGPU_DIR");
    println!("cargo:rustc-link-search=native={dir}");
    println!("cargo:rustc-link-lib=dylib=pearlgpu");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN:{dir}");
}
