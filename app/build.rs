fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "SwitchDesk");
        res.set("FileDescription", "SwitchDesk");
        if let Err(e) = res.compile() {
            println!("cargo:warning=не удалось встроить ресурсы: {e}");
        }
    }
}
