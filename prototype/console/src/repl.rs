//! The terminal loop: prompt, commands, streamed answers.
//!
//! Everything here is I/O; behaviour lives in [`crate::app::App`]. The loop is
//! deliberately synchronous: the prototype's model backend is a blocking HTTP
//! client, and a streamed answer needs no async runtime to feel alive.

use std::io::{self, Write};

use crate::app::App;
use crate::error::ConsoleError;

/// Runs the REPL until `/quit` or Ctrl-D.
pub fn run(app: &mut App) -> Result<(), ConsoleError> {
    let stdin = io::stdin();
    let mut out = io::stdout();
    loop {
        write!(out, "{}", app.prompt())?;
        out.flush()?;

        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            // Ctrl-D: leave cleanly and close the session.
            writeln!(out)?;
            app.finish("eof")?;
            return Ok(());
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(command) = line.strip_prefix('/') {
            let action = app.handle_command(&format!("/{command}"))?;
            if !action.output.is_empty() {
                writeln!(out, "{}", action.output)?;
            }
            out.flush()?;
            if action.quit {
                return Ok(());
            }
            continue;
        }

        let result = {
            let out = &mut out;
            app.send(line, &mut |token| {
                let _ = write!(out, "{token}");
                let _ = out.flush();
            })
        };
        writeln!(out)?;
        match result {
            Ok(reply) => {
                if let Some(stats) = reply.stats {
                    writeln!(
                        out,
                        "[токены: prompt {} · completion {}]",
                        stats.prompt_tokens, stats.completion_tokens
                    )?;
                }
            }
            Err(error) => writeln!(out, "ошибка: {error}")?,
        }
        out.flush()?;
    }
}
