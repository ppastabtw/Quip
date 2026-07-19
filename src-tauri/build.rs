fn main() {
    #[cfg(target_os = "macos")]
    cc::Build::new()
        .file("src/input_method_shim.m")
        .flag("-fobjc-arc")
        .compile("quip_input_method_shim");
    tauri_build::build()
}
