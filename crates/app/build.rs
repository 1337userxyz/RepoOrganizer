fn main() {
    kit_theme::compile_slint("ui/main.slint");

    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=../../assets/repoorganizer.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/repoorganizer.ico");
        res.compile().expect("Windows resource compilation failed");
    }
}
