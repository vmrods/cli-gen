use std::io::{self, BufRead, IsTerminal, Write};

use crate::error::CliError;

/// Prompts only make sense when a person can answer them (ADR 0002).
pub fn is_interactive() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

/// Asks on stderr and reads one trimmed line from stdin.
pub fn ask(question: &str) -> Result<String, CliError> {
    ask_with(&mut io::stdin().lock(), &mut io::stderr(), question)
}

/// Asks a yes/no question; anything but `y`/`yes` means no.
pub fn confirm(question: &str) -> Result<bool, CliError> {
    let answer = ask(&format!("{question} [y/N] "))?;
    Ok(is_yes(&answer))
}

fn ask_with(
    input: &mut impl BufRead,
    output: &mut impl Write,
    question: &str,
) -> Result<String, CliError> {
    output
        .write_all(question.as_bytes())
        .and_then(|()| output.flush())
        .map_err(CliError::Terminal)?;
    let mut line = String::new();
    input.read_line(&mut line).map_err(CliError::Terminal)?;
    Ok(line.trim().to_owned())
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn ask_with_writes_question_and_returns_trimmed_answer() {
        let mut output = Vec::new();
        let answer = ask_with(&mut Cursor::new("  ./api.json \n"), &mut output, "Spec: ").unwrap();
        assert_eq!(answer, "./api.json");
        assert_eq!(output, b"Spec: ");
    }

    #[test]
    fn ask_with_returns_empty_string_at_end_of_input() {
        let answer = ask_with(&mut Cursor::new(""), &mut Vec::new(), "Spec: ").unwrap();
        assert_eq!(answer, "");
    }

    #[test]
    fn is_yes_accepts_only_explicit_yes() {
        assert!(is_yes("y"));
        assert!(is_yes("YES"));
        assert!(!is_yes(""));
        assert!(!is_yes("n"));
        assert!(!is_yes("yep"));
    }
}
