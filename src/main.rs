fn main() {
    if let Err(error) = protoswitch::run() {
        eprintln!("ProtoSwitch: {error:#}");
        std::process::exit(1);
    }
}
