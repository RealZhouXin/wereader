mod software;

use slint::ComponentHandle;
use std::{env, error::Error, path::PathBuf};

slint::include_modules!();

const DEFAULT_FONT: &[u8] = include_bytes!("../assets/Tuffy.ttf");

#[derive(Default, Debug)]
struct Options {
    device: bool,
    output: Option<PathBuf>,
    font: Option<PathBuf>,
    black_and_white: bool,
    rotation: u32,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, Box<dyn Error>> {
        let mut opts = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--device" => opts.device = true,
                "--black-and-white" => opts.black_and_white = true,
                "--output" | "--font" | "--rotation" => {
                    let value = args.next().ok_or("missing option value")?;
                    match arg.as_str() {
                        "--output" => opts.output = Some(value.into()),
                        "--font" => opts.font = Some(value.into()),
                        _ => opts.rotation = value.parse()?,
                    }
                }
                _ => return Err(format!("unknown argument: {arg}; see --help").into()),
            }
        }
        if opts.device && opts.output.is_some() {
            return Err("--output cannot be used with --device".into());
        }
        if ![0, 90, 180, 270].contains(&opts.rotation) {
            return Err("--rotation must be 0, 90, 180 or 270".into());
        }
        if !opts.device && (opts.font.is_some() || opts.black_and_white || opts.rotation != 0) {
            return Err("--font, --black-and-white and --rotation require --device".into());
        }
        Ok(opts)
    }
}

fn create_ui() -> Result<AppWindow, slint::PlatformError> {
    let ui = AppWindow::new()?;
    let weak = ui.as_weak();
    ui.on_next_page(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_page(ui.get_page().saturating_add(1));
        }
    });
    let weak = ui.as_weak();
    ui.on_previous_page(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_page(ui.get_page().saturating_sub(1).max(1));
        }
    });
    ui.on_exit(|| {
        if let Err(error) = slint::quit_event_loop() {
            eprintln!("failed to quit event loop: {error}");
        }
    });
    Ok(ui)
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
fn run_kindle(opts: &Options) -> Result<(), Box<dyn Error>> {
    let font = opts.font.as_ref().map(std::fs::read).transpose()?;
    let backend = slint_backend_kindle::install(font.as_deref().unwrap_or(DEFAULT_FONT))?;
    backend.set_black_and_white(opts.black_and_white);
    backend.set_rotation(opts.rotation);
    let ui = create_ui()?;
    // Ensure the backend's first poll has a timer deadline, even without touch.
    slint::Timer::single_shot(std::time::Duration::ZERO, || {});
    ui.run()?;
    Ok(())
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("rereader                         Desktop Slint window\nrereader --output preview.pgm    Headless Slint screenshot\nrereader --device [--font path.ttf] [--black-and-white] [--rotation 90]\nKindle mode requires a Linux build with --no-default-features --features kindle.");
        return Ok(());
    }
    let opts = Options::parse(args)?;
    if opts.device {
        #[cfg(all(feature = "kindle", target_os = "linux"))]
        return run_kindle(&opts);
        #[cfg(not(all(feature = "kindle", target_os = "linux")))]
        return Err(
            "Kindle mode requires Linux and --no-default-features --features kindle".into(),
        );
    }
    if let Some(output) = opts.output {
        return software::screenshot(&output);
    }
    #[cfg(feature = "desktop")]
    {
        create_ui()?.run()?;
        Ok(())
    }
    #[cfg(not(feature = "desktop"))]
    Err("desktop backend disabled; use --device or --output".into())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("rereader: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Options, Box<dyn Error>> {
        Options::parse(args.iter().map(|s| s.to_string()))
    }
    #[test]
    fn validates_backend_options() {
        assert!(parse(&["--device", "--rotation", "90"]).is_ok());
        assert!(parse(&["--device", "--rotation", "45"]).is_err());
        assert!(parse(&["--device", "--output", "preview.pgm"]).is_err());
        assert!(parse(&["--font", "font.ttf"]).is_err());
        assert!(parse(&["--output"]).is_err());
    }
}
