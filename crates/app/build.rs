fn main() {
    slint_build::compile_with_config("ui/main.slint", slint_build::CompilerConfiguration::new().with_style("fluent".into())).unwrap();
    println!("cargo:rerun-if-changed=ui/main.slint");
    #[cfg(target_os = "linux")]
    {
        println!("cargo:rustc-link-lib=dbus-1");
    }
}

