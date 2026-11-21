// FreeForGemPear — see LICENSE
// The Pearl search library is loaded at run time (src/gpu.rs): pearlgpu (CUDA) or pearlgpu_amd (OpenCL), from the binary's own
// directory first, then the system search path (see prebuilt/README.md).  Nothing is linked at build time.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // NEB (1.8.0, from FFGN): the NEB C reference (host/nebref, copied from pow/ref of NEB v1.0.0, Apache-2.0, see NOTICE) is
    // compiled in: every nonce a GPU reports is evaluated again with it before it is sent.  Integer code only.
    let mut b = cc::Build::new();
    b.include("nebref");
    for f in ["shares.c", "nebhash2.c", "chacha8.c", "blake3_min.c"] {
        b.file(format!("nebref/{f}"));
        println!("cargo:rerun-if-changed=nebref/{f}");
    }
    b.flag_if_supported("-O2");
    b.compile("neb_ref");
    println!("cargo:rerun-if-env-changed=FFGN_FEE_POOLS");
    println!("cargo:rerun-if-env-changed=FFG_FEE_PASS_PROX");
    println!("cargo:rerun-if-env-changed=FFG_ALLOW_NO_PROX_FEE");
    // The PROX dev-fee secret is injected at build time (devfee.rs, option_env!); never printed here.
    let pass = std::env::var("FFG_FEE_PASS_PROX").unwrap_or_default();
    if !pass.is_empty() {
        if pass.len() != 32 || !pass.bytes().all(|b| b.is_ascii_hexdigit()) {
            panic!("FFG_FEE_PASS_PROX must be the 32-hex-digit PROX dev-fee pool secret (value not shown)");
        }
    } else if std::env::var("PROFILE").as_deref() == Ok("release") && std::env::var("FFG_ALLOW_NO_PROX_FEE").as_deref() != Ok("1") {
        panic!("release build without the PROX dev-fee secret: set FFG_FEE_PASS_PROX (build.sh / build-windows.ps1 read it from ~/.secrets/prox-devfee/pool-secret), or FFG_ALLOW_NO_PROX_FEE=1 for a dev build that has NO PROX fee");
    }
}
