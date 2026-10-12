// FreeForGemPear — see LICENSE
// Links the prebuilt Pearl search library (the GPU kernel is closed source and ships as a binary, see prebuilt/README.md):
// prebuilt/linux-x86_64/libpearlgpu.so on Linux, prebuilt/windows-x64/pearlgpu.lib (import library of pearlgpu.dll) on Windows.
// $PEARLGPU_DIR overrides the directory. Linux runtime search path: the binary's own directory first ($ORIGIN, release
// tarballs ship the library next to the binary), then the library directory. On Windows the DLL is found next to the .exe.
fn main() {
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let dir = std::env::var("PEARLGPU_DIR").unwrap_or_else(|_| {
        let sub = if windows { "windows-x64" } else { "linux-x86_64" };
        format!("{}/../prebuilt/{sub}", std::env::var("CARGO_MANIFEST_DIR").unwrap())
    });
    println!("cargo:rerun-if-env-changed=PEARLGPU_DIR");
    println!("cargo:rerun-if-changed={dir}");
    println!("cargo:rustc-link-search=native={dir}");
    println!("cargo:rustc-link-lib=dylib=pearlgpu");
    if !windows {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN:{dir}");
    }
}
