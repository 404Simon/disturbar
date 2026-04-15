pub enum Mode {
    Ui,
    InputBackend,
}

pub fn parse_mode(args: impl IntoIterator<Item = String>) -> Result<Mode, String> {
    let args: Vec<String> = args.into_iter().collect();
    match args.as_slice() {
        [] => Ok(Mode::Ui),
        [arg] if arg == "--input-backend" => Ok(Mode::InputBackend),
        _ => Err("usage: disturbar [--input-backend]".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, parse_mode};

    #[test]
    fn parse_mode_defaults_to_ui() {
        let result = parse_mode(Vec::<String>::new());
        assert!(matches!(result, Ok(Mode::Ui)));
    }

    #[test]
    fn parse_mode_accepts_input_backend_flag() {
        let result = parse_mode(vec!["--input-backend".to_string()]);
        assert!(matches!(result, Ok(Mode::InputBackend)));
    }

    #[test]
    fn parse_mode_rejects_unknown_args() {
        let result = parse_mode(vec!["--wat".to_string()]);
        assert!(matches!(result, Err(msg) if msg == "usage: disturbar [--input-backend]"));
    }

    #[test]
    fn parse_mode_rejects_extra_args() {
        let result = parse_mode(vec!["--input-backend".to_string(), "extra".to_string()]);
        assert!(matches!(result, Err(msg) if msg == "usage: disturbar [--input-backend]"));
    }
}
