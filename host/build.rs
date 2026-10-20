// FreeForGemPear — see LICENSE
// The Pearl search library is loaded at run time (src/gpu.rs): pearlgpu (CUDA) or pearlgpu_amd (OpenCL), from the binary's own
// directory first, then the system search path (see prebuilt/README.md).  Nothing is linked at build time.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
