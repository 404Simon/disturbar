mod cli;
mod constants;
mod input_backend;
mod process;
mod renderer;
mod signals;
mod status;
mod wayland_ui;

use cli::Mode;

fn main() {
    match cli::parse_mode(std::env::args().skip(1)) {
        Ok(Mode::Ui) => {
            if let Err(err) = wayland_ui::run_wayland_bar() {
                eprintln!("disturbar: wayland bar failed: {err}");
                std::process::exit(1);
            }
        }
        Ok(Mode::InputBackend) => input_backend::run_input_backend(),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    }
}
