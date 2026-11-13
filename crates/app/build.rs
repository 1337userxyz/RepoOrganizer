fn main() {
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_library_paths(kit_theme::library_paths());
    slint_build::compile_with_config("ui/main.slint", config).unwrap();
    println!("cargo:rerun-if-changed=ui/main.slint");
    #[cfg(target_os = "linux")]
    {
        println!("cargo:rustc-link-lib=dbus-1");
    }
}

