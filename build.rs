fn main() {
    // musl implements dl* in libc. Rust's self-contained sysroot includes libc.a
    // but omits musl's empty libdl.a compatibility archive. libloading (used by
    // fontique) still passes -ldl. Supply the same empty archive locally; actual
    // symbols must still resolve from libc, so missing symbols remain errors.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl") {
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        std::fs::write(out.join("libdl.a"), b"!<arch>\n")
            .expect("write musl compatibility archive");
        println!("cargo:rustc-link-search=native={}", out.display());
    }
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    slint_build::compile_with_config("ui/app.slint", config).expect("compile Slint UI");
}
