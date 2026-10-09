//! Command lines that bring a session back after the daemon restarts, for example
//! after a reboot (DESIGN §8.3). An agent resumes its own conversation; its options
//! are kept, and the prompt it started with is dropped so nothing is sent twice.

/// Options and positional arguments of `args` (after the program name).
struct Split {
    options: Vec<String>,
    positionals: Vec<String>,
}

/// Flag tables for one CLI. A name ending in `...` takes values up to the next flag.
struct Flags<'a> {
    /// Take a value: `--model opus`, `-s read-only`. `--x=v` is handled anyway.
    value: &'a [&'a str],
    /// Dropped: they pick or start a conversation, which the restore decides.
    drop: &'a [&'a str],
    /// Dropped with their value, when it follows as a separate argument.
    drop_value: &'a [&'a str],
}

fn split(args: &[String], flags: &Flags) -> Split {
    let mut split = Split {
        options: Vec::new(),
        positionals: Vec::new(),
    };
    let named = |list: &[&str], name: &str| {
        list.iter()
            .find(|f| f.trim_end_matches("...") == name)
            .map(|f| f.ends_with("..."))
    };
    let is_flag = |s: &str| s.len() > 1 && s.starts_with('-');
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg == "--" {
            split.positionals.extend(args[i..].iter().cloned());
            break;
        }
        if !is_flag(arg) {
            split.positionals.push(arg.clone());
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((name, _)) => (name, true),
            None => (arg.as_str(), false),
        };
        if named(flags.drop, name).is_some() {
            continue;
        }
        let follows = |i: usize| !inline && i < args.len() && !is_flag(&args[i]);
        if named(flags.drop_value, name).is_some() {
            if follows(i) {
                i += 1;
            }
            continue;
        }
        split.options.push(arg.clone());
        if let Some(many) = named(flags.value, name) {
            while follows(i) {
                split.options.push(args[i].clone());
                i += 1;
                if !many {
                    break;
                }
            }
        }
    }
    split
}

const CLAUDE: Flags = Flags {
    value: &[
        "--add-dir...",
        "--agent",
        "--append-system-prompt-file",
        "--max-turns",
        "--permission-prompt-tool",
        "--system-prompt-file",
        "--agents",
        "--allowedTools...",
        "--allowed-tools...",
        "--append-system-prompt",
        "--autocompact",
        "--betas...",
        "--debug-file",
        "--disallowedTools...",
        "--disallowed-tools...",
        "--effort",
        "--environment",
        "--fallback-model",
        "--file...",
        "--input-format",
        "--json-schema",
        "--max-budget-usd",
        "--mcp-config...",
        "--model",
        "-n",
        "--name",
        "--output-format",
        "--permission-mode",
        "--permission-prompts",
        "--plugin-dir",
        "--plugin-url",
        "--remote-control-session-name-prefix",
        "--setting-sources",
        "--settings",
        "--system-prompt",
        "--system-prompt-snapshot",
        "--tools...",
        // Optional values: a word after them is theirs, not a prompt.
        "-d",
        "--debug",
        "--prompt-suggestions",
        "--remote-control",
    ],
    // Setup hooks ran once; the session's cwd is already its worktree.
    drop: &[
        "-c",
        "--continue",
        "--fork-session",
        "--init",
        "--init-only",
        "--maintenance",
    ],
    drop_value: &[
        "-r",
        "--resume",
        "--session-id",
        "--from-pr",
        "--teleport",
        "--cloud",
        "-w",
        "--worktree",
    ],
};

/// `claude --resume <id> [options]`. The id comes first: a value-taking flag missing
/// from the table keeps no value, and at the end it would swallow `--resume` and send
/// the id as a prompt; here it fails loudly instead. A `-p` run was one-shot.
pub fn claude(command: &[String], conversation: Option<&str>) -> Option<Vec<String>> {
    let id = conversation?;
    let (program, args) = command.split_first()?;
    if args.iter().any(|a| a == "-p" || a == "--print") {
        return None;
    }
    let split = split(args, &CLAUDE);
    let mut out = vec![program.clone(), "--resume".to_owned(), id.to_owned()];
    out.extend(split.options);
    Some(out)
}

const CODEX: Flags = Flags {
    value: &[
        "-c",
        "--config",
        "--enable",
        "--disable",
        "--remote",
        "--remote-auth-token-env",
        "-m",
        "--model",
        "--local-provider",
        "-p",
        "--profile",
        "-s",
        "--sandbox",
        "-C",
        "--cd",
        "--add-dir",
        "-a",
        "--ask-for-approval",
    ],
    drop: &["--last"],
    // Images belong to the first prompt.
    drop_value: &["-i", "--image"],
};

/// `codex resume <id> [options]`. Only the interactive CLI is restored: a command
/// like `codex exec` ran once and is not a session to come back to.
pub fn codex(command: &[String], conversation: Option<&str>) -> Option<Vec<String>> {
    let id = conversation?;
    let (program, args) = command.split_first()?;
    let split = split(args, &CODEX);
    // `codex resume <old id>` is interactive too; anything else first is a prompt
    // unless it names another subcommand.
    if let Some(first) = split.positionals.first()
        && first != "resume"
        && SUBCOMMANDS.contains(&first.as_str())
    {
        return None;
    }
    // `resume` takes the same options; the id first, as for Claude.
    let mut out = vec![program.clone(), "resume".to_owned(), id.to_owned()];
    out.extend(split.options);
    Some(out)
}

const SUBCOMMANDS: &[&str] = &[
    "agents",
    "exec",
    "e",
    "review",
    "login",
    "logout",
    "mcp",
    "plugin",
    "app-server",
    "remote-control",
    "completion",
    "update",
    "doctor",
    "sandbox",
    "debug",
    "apply",
    "a",
    "resume",
    "queue",
    "help",
];

/// An interactive shell comes back as it was started, in the same directory.
pub fn shell(command: &[String]) -> Option<Vec<String>> {
    let program = crate::program_name(command.first()?)?;
    let shells = [
        "sh",
        "bash",
        "zsh",
        "fish",
        "nu",
        "dash",
        "ksh",
        "tcsh",
        "elvish",
        "xonsh",
        "pwsh",
        "powershell",
        "cmd",
    ];
    // Only flags known to keep a shell interactive: `bash -c 'make deploy'`,
    // `bash -lc …` and `fish --command=…` are commands, not shells to restore.
    const INTERACTIVE: &[&str] = &[
        "-l",
        "-i",
        "-li",
        "-il",
        "--login",
        "--interactive",
        "--norc",
        "--noprofile",
        "--posix",
        "--private",
        "-P",
        "-N",
        "--no-config",
        // PowerShell's.
        "-NoLogo",
        "-NoProfile",
        "-NoExit",
    ];
    let interactive = command[1..]
        .iter()
        .all(|a| INTERACTIVE.contains(&a.as_str()));
    (shells.contains(&program) && interactive).then(|| command.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    #[test]
    fn claude_keeps_options_and_drops_the_prompt() {
        assert_eq!(
            claude(&v("claude"), Some("abc")),
            Some(v("claude --resume abc"))
        );
        assert_eq!(
            claude(
                &v("/usr/bin/claude --model opus --dangerously-skip-permissions fix-the-bug"),
                Some("abc")
            ),
            Some(v(
                "/usr/bin/claude --resume abc --model opus --dangerously-skip-permissions"
            ))
        );
        assert_eq!(
            claude(
                &v("claude --add-dir a b --permission-mode=plan -c -w"),
                Some("x")
            ),
            Some(v("claude --resume x --add-dir a b --permission-mode=plan"))
        );
        // An unknown flag loses its value but cannot swallow the resume.
        assert_eq!(
            claude(&v("claude --new-flag val"), Some("x")),
            Some(v("claude --resume x --new-flag"))
        );
        // A print run was one-shot.
        assert_eq!(claude(&v("claude -p hello"), Some("x")), None);
        // A conversation picked at start is replaced by the one it became.
        assert_eq!(
            claude(&v("claude -r old --session-id 123 -- prompt"), Some("new")),
            Some(v("claude --resume new"))
        );
        assert_eq!(claude(&v("claude"), None), None);
    }

    #[test]
    fn codex_resumes_the_interactive_cli_only() {
        assert_eq!(
            codex(&v("codex -s read-only -a on-request do-things"), Some("u1")),
            Some(v("codex resume u1 -s read-only -a on-request"))
        );
        assert_eq!(
            codex(&v("codex resume --last"), Some("u2")),
            Some(v("codex resume u2"))
        );
        assert_eq!(
            codex(&v("codex -i shot.png explain"), Some("u3")),
            Some(v("codex resume u3"))
        );
        assert_eq!(codex(&v("codex exec run-tests"), Some("u4")), None);
    }

    #[test]
    fn only_interactive_shells_restore() {
        assert_eq!(shell(&v("/bin/bash")), Some(v("/bin/bash")));
        assert_eq!(shell(&v("fish -l")), Some(v("fish -l")));
        assert_eq!(shell(&v("bash -c make")), None);
        assert_eq!(shell(&v("bash script.sh")), None);
        assert_eq!(shell(&v("bash -lc make")), None);
        assert_eq!(shell(&v("fish --command=make")), None);
        assert_eq!(shell(&v("htop")), None);
    }
}
