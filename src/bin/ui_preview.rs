use clap::Parser;

#[derive(Parser)]
struct PreviewArgs {
    #[arg(long, default_value_t = 120)]
    width: u16,
    #[arg(long, default_value_t = 34)]
    height: u16,
    #[arg(long, default_value = "dashboard")]
    section: String,
    #[arg(long)]
    sample: bool,
}

fn main() {
    let args = PreviewArgs::parse();
    match protoswitch::render_ui_preview(args.width, args.height, &args.section, args.sample) {
        Ok(rendered) => {
            print!("{rendered}");
        }
        Err(error) => {
            eprintln!("ui_preview: {error:#}");
            std::process::exit(1);
        }
    }
}
