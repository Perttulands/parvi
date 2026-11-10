use super::adapter::{Launch, Shell, SlotBinding, TranscriptRoots};
use parvi_term::waiting::harness::Harness;
use std::{io, path::Path};
fn quote(s: &str, shell: Shell) -> String {
    match shell {
        Shell::Posix => format!("'{}'", s.replace('\'', "'\\''")),
        Shell::PowerShell => format!("'{}'", s.replace('\'', "''")),
    }
}
pub(super) fn render(
    harness: Harness,
    binding: &SlotBinding,
    cwd: &Path,
    shell: Shell,
    roots: &TranscriptRoots,
) -> io::Result<Launch> {
    let cwd = cwd.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "non-Unicode working directory")
    })?;
    let effort = if binding.effort.is_empty() {
        "medium"
    } else {
        &binding.effort
    };
    let bin = match harness {
        Harness::Claude => "claude",
        Harness::Codex => "codex",
        Harness::Pi => "pi",
    };
    let mut argv = vec![bin.to_owned()];
    if !binding.model.is_empty() {
        argv.extend(["--model".into(), binding.model.clone()]);
    }
    match harness {
        Harness::Claude => argv.extend([
            "--effort".into(),
            effort.into(),
            "--dangerously-skip-permissions".into(),
        ]),
        Harness::Codex => {
            let project = format!(
                "projects={{{}={{trust_level=\"trusted\"}}}}",
                serde_json::to_string(cwd)?
            );
            argv.extend([
                "-c".into(),
                format!("model_reasoning_effort=\"{effort}\""),
                "-c".into(),
                "check_for_update_on_startup=false".into(),
                "--dangerously-bypass-approvals-and-sandbox".into(),
                "-c".into(),
                project,
            ]);
        }
        Harness::Pi => {
            let dir = cwd.trim_start_matches('/').replace(['/', '\\', ':'], "-");
            let dir = roots.pi.join(format!("--{dir}--"));
            let dir = dir.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "non-Unicode Pi session directory",
                )
            })?;
            argv.extend([
                "--thinking".into(),
                effort.into(),
                "--approve".into(),
                "--session-dir".into(),
                dir.into(),
            ]);
        }
    }
    let prefix = if shell == Shell::Posix { "exec " } else { "& " };
    let command = format!(
        "{prefix}{}",
        argv.iter()
            .map(|arg| quote(arg, shell))
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok(Launch { command, argv })
}
