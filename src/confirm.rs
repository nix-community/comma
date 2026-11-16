use std::{
    fs::OpenOptions,
    io::{self, BufRead, BufReader, Write},
};

use crate::run::EXIT_NOT_FOUND;

/// The exit status when the user declined to run the command. It is what a
/// shell gives for a command that wasn't found, so that after a declined
/// command-not-found `typo && other` doesn't go on to run `other`
pub const EXIT_NOT_RUN: u8 = EXIT_NOT_FOUND;

/// Ask whether to run `command` from the package `derivation`, with the
/// question written to `output` and the answer read from `input`. This is
/// asked before the package is built, so its store path isn't known yet
fn confirm(
    input: &mut impl BufRead,
    output: &mut impl Write,
    command: &str,
    derivation: &str,
) -> io::Result<bool> {
    loop {
        write!(output, "Run '{command}' from '{derivation}'? [Y/n]: ")?;
        output.flush()?;

        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            // The input ended without an answer (Ctrl-D). Only an empty line
            // means yes. End the line of the question, as Enter would have
            writeln!(output)?;
            return Ok(false);
        }

        match answer.trim().to_lowercase().as_str() {
            "y" | "yes" | "" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => writeln!(output, "Please enter 'y' or 'n'.")?,
        }
    }
}

/// Ask on the terminal whether to run `command` from the package
/// `derivation`. Without a terminal to ask on, the answer is no
pub fn confirmer(command: &str, derivation: &str) -> bool {
    // stdin and stdout belong to the command and may be pipes
    // (`cat file | cmd`, `cmd | less`), and a command-not-found handler may
    // capture stdout. So ask on the terminal itself, like ssh and sudo do
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .and_then(|tty| confirm(&mut BufReader::new(&tty), &mut &tty, command, derivation))
        .unwrap_or_else(|err| {
            eprintln!(
                "comma: not running '{command}' from '{derivation}': \
                 can't ask for confirmation on the terminal: {err}"
            );
            false
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The result of `confirm` for the given input, and what it wrote
    fn confirm_with(input: &str) -> (bool, String) {
        let mut output = Vec::new();
        let answer = confirm(&mut input.as_bytes(), &mut output, "sl", "sl.out").unwrap();
        (answer, String::from_utf8(output).unwrap())
    }

    #[test]
    fn confirm_answers() {
        let question = "Run 'sl' from 'sl.out'? [Y/n]: ";

        // An empty line is the default, yes
        assert_eq!(confirm_with("y\n"), (true, question.to_owned()));
        assert_eq!(confirm_with("\n"), (true, question.to_owned()));
        assert_eq!(confirm_with("n\n"), (false, question.to_owned()));

        // Without an answer the program isn't run
        assert_eq!(confirm_with(""), (false, format!("{question}\n")));

        // Asked again after anything else
        assert_eq!(
            confirm_with("maybe\ny\n"),
            (
                true,
                format!("{question}Please enter 'y' or 'n'.\n{question}")
            )
        );
    }
}
