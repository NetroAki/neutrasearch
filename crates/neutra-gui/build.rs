fn main() {
    println!("cargo:rerun-if-changed=assets/neutrasearch.ico");

    #[cfg(target_os = "windows")]
    winresource::WindowsResource::new()
        .set_icon("assets/neutrasearch.ico")
        .compile()
        .expect("embed Neutrasearch Windows icon");
}
