//! Small stdin-reading helpers shared by every subcommand that accepts
//! either a file path or piped/typed stdin input.

use std::io;

/// Reads a single line from stdin.
pub fn read_line() -> io::Result<String> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(input)
}

/// Reads lines from stdin until a blank line, joining them with `\n`. Used
/// as the fallback input source for commands that also accept a file path.
pub fn read_lines_from_stdin() -> io::Result<String> {
    let mut lines: Vec<String> = Vec::new();

    let mut line = read_line()?;
    while !line.trim().is_empty() {
        lines.push(line);
        line = read_line()?;
    }

    Ok(lines.join("\n"))
}
