//! `waserver edit`: opens a share's `share.toml` in the user's editor, then
//! checks the result and applies it to the running server.

use crate::ActionOutcome;
use crate::storage;
use anyhow::{Context, Result};
use std::io::{self, BufRead, Write};

pub async fn edit(share: &str) -> Result<()> {
    let dir = storage::ShareDir::new(share)?;
    if !dir.exists() {
        anyhow::bail!("Share {} is not initialised", share);
    }
    let path = dir.config_path();
    loop {
        open_in_editor(&path)?;
        match dir.load_config() {
            Ok(_) => break,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                if !asks_yes("Edit again? [Y/n] ")? {
                    anyhow::bail!("left as written. The server keeps its running config");
                }
            }
        }
    }
    match crate::reload_share(share).await? {
        ActionOutcome::Done(what) => println!("{what}"),
        ActionOutcome::NotRunning => println!("Saved."),
    }
    Ok(())
}

/// Runs the user's editor on the file and waits for it to exit. The editor is
/// `$VISUAL`, else `$EDITOR`, else Debian's `sensible-editor` where it exists,
/// else `vi` (`notepad` on Windows). A variable may carry arguments, as in
/// `code --wait`.
fn open_in_editor(path: &std::path::Path) -> Result<()> {
    let mut command = editor_command();
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command
        .arg(path)
        .status()
        .with_context(|| format!("cannot run the editor '{program}'"))?;
    if !status.success() {
        anyhow::bail!("the editor '{program}' failed ({status})");
    }
    Ok(())
}

fn editor_command() -> std::process::Command {
    for var in ["VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(var) {
            let mut words = value.split_whitespace();
            if let Some(program) = words.next() {
                let mut command = std::process::Command::new(program);
                command.args(words);
                return command;
            }
        }
    }
    let program = if cfg!(windows) {
        "notepad"
    } else if std::path::Path::new("/usr/bin/sensible-editor").exists() {
        "sensible-editor"
    } else {
        "vi"
    };
    std::process::Command::new(program)
}

/// Prints the prompt and reads one line. Empty or `y` means yes. A closed
/// stdin means no, so a script that breaks the file cannot loop forever.
fn asks_yes(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line)? == 0 {
        println!();
        return Ok(false);
    }
    let answer = line.trim().to_ascii_lowercase();
    Ok(answer.is_empty() || answer == "y" || answer == "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_comes_from_the_environment_first() {
        // Environment variables are process-wide; set and restore around the check.
        unsafe {
            std::env::set_var("VISUAL", "code --wait");
            std::env::set_var("EDITOR", "nano");
        }
        assert_eq!(words(&editor_command()), ["code", "--wait"]);
        unsafe { std::env::set_var("VISUAL", "  ") }
        assert_eq!(words(&editor_command()), ["nano"]);
        unsafe {
            std::env::remove_var("VISUAL");
            std::env::remove_var("EDITOR");
        }
        let fallback = words(&editor_command());
        assert!(["sensible-editor", "vi", "notepad"].contains(&fallback[0].as_str()));
        assert_eq!(fallback.len(), 1);
    }

    /// The command line a `Command` would run, program first.
    fn words(command: &std::process::Command) -> Vec<String> {
        std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|w| w.to_string_lossy().into_owned())
            .collect()
    }
}
