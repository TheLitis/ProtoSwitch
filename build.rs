fn main() {
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/windows/protoswitch.ico");
        resource.set("FileDescription", "ProtoSwitch");
        resource.set("ProductName", "ProtoSwitch");
        resource.set("CompanyName", "The_Litis");
        resource.set("InternalName", "ProtoSwitch");
        resource
            .compile()
            .expect("failed to compile windows resources");
    }
}
