use clap::Parser;

#[derive(clap::ValueEnum, Clone, Copy)]
enum PreviewFormat {
    Text,
    Json,
}

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
    #[arg(long, value_enum, default_value_t = PreviewFormat::Text)]
    format: PreviewFormat,
}

fn main() {
    let args = PreviewArgs::parse();
    let result = match args.format {
        PreviewFormat::Text => {
            protoswitch::render_ui_preview(args.width, args.height, &args.section, args.sample)
        }
        PreviewFormat::Json => {
            protoswitch::render_ui_preview_json(args.width, args.height, &args.section, args.sample)
        }
    };

    match result {
        Ok(rendered) => {
            print!("{rendered}");
        }
        Err(error) => {
            eprintln!("ui_preview: {error:#}");
            std::process::exit(1);
        }
    }
}
