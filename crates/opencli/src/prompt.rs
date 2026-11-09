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

/// Like [`ask`], but the end of input is an error rather than an empty
/// answer, so a loop that re-asks for a required value can't spin forever.
pub fn ask_line(question: &str) -> Result<String, CliError> {
    ask_line_with(&mut io::stdin().lock(), &mut io::stderr(), question)
}

/// Asks a yes/no question; anything but `y`/`yes` means no.
pub fn confirm(question: &str) -> Result<bool, CliError> {
    let answer = ask(&format!("{question} [y/N] "))?;
    Ok(is_yes(&answer))
}

/// Asks a yes/no question where Enter means yes.
pub fn confirm_default_yes(question: &str) -> Result<bool, CliError> {
    let answer = ask(&format!("{question} [Y/n] "))?;
    Ok(answer.is_empty() || is_yes(&answer))
}

/// Lists `options` on stderr and asks for a number; returns the chosen index.
pub fn choose(question: &str, options: &[String]) -> Result<usize, CliError> {
    choose_with(
        &mut io::stdin().lock(),
        &mut io::stderr(),
        question,
        options,
    )
}

/// Lists numbered `options` on stderr, then asks; the raw answer is returned.
pub fn ask_listing(question: &str, options: &[String]) -> Result<String, CliError> {
    let mut stderr = io::stderr();
    for (number, option) in (1..).zip(options) {
        writeln!(stderr, "  {number}) {option}").map_err(CliError::Terminal)?;
    }
    ask(question)
}

/// One line of feedback on stderr, e.g. why an answer was rejected.
pub fn say(message: &str) -> Result<(), CliError> {
    writeln!(io::stderr(), "{message}").map_err(CliError::Terminal)
}

/// Reads a secret without echoing it.
pub fn ask_secret(question: &str) -> Result<String, CliError> {
    rpassword::prompt_password(question).map_err(CliError::Terminal)
}

fn choose_with(
    input: &mut impl BufRead,
    output: &mut impl Write,
    question: &str,
    options: &[String],
) -> Result<usize, CliError> {
    for (number, option) in (1..).zip(options) {
        writeln!(output, "  {number}) {option}").map_err(CliError::Terminal)?;
    }
    loop {
        let answer = ask_with(
            input,
            output,
            &format!("{question} [1-{}]: ", options.len()),
        )?;
        if answer.is_empty() {
            return Err(CliError::Terminal(io::Error::from(
                io::ErrorKind::UnexpectedEof,
            )));
        }
        match answer.parse::<usize>() {
            Ok(number) if (1..=options.len()).contains(&number) => return Ok(number - 1),
            _ => writeln!(output, "Enter a number from 1 to {}.", options.len())
                .map_err(CliError::Terminal)?,
        }
    }
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

fn ask_line_with(
    input: &mut impl BufRead,
    output: &mut impl Write,
    question: &str,
) -> Result<String, CliError> {
    output
        .write_all(question.as_bytes())
        .and_then(|()| output.flush())
        .map_err(CliError::Terminal)?;
    let mut line = String::new();
    if input.read_line(&mut line).map_err(CliError::Terminal)? == 0 {
        return Err(CliError::Terminal(io::Error::from(
            io::ErrorKind::UnexpectedEof,
        )));
    }
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
    fn ask_line_with_fails_at_end_of_input_but_accepts_an_empty_line() {
        assert!(ask_line_with(&mut Cursor::new(""), &mut Vec::new(), "Id: ").is_err());
        let answer = ask_line_with(&mut Cursor::new("\n"), &mut Vec::new(), "Id: ").unwrap();
        assert_eq!(answer, "");
    }

    #[test]
    fn choose_with_lists_options_and_retries_until_valid() {
        let options = vec!["apiKey (api-key)".to_owned(), "oauth (oauth2)".to_owned()];
        let mut output = Vec::new();
        let index = choose_with(
            &mut Cursor::new("x\n9\n2\n"),
            &mut output,
            "Scheme",
            &options,
        )
        .unwrap();
        assert_eq!(index, 1);
        let shown = String::from_utf8(output).unwrap();
        assert!(
            shown.starts_with("  1) apiKey (api-key)\n  2) oauth (oauth2)\nScheme [1-2]: "),
            "{shown}"
        );
        assert_eq!(shown.matches("Enter a number from 1 to 2.").count(), 2);
    }

    #[test]
    fn choose_with_gives_up_at_end_of_input() {
        let options = vec!["a".to_owned()];
        assert!(choose_with(&mut Cursor::new(""), &mut Vec::new(), "Pick", &options).is_err());
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
