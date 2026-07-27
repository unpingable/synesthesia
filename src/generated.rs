use std::io::{self, Write};

use clap::CommandFactory;
use clap_complete::{Shell, generate};
use clap_mangen::{
    Man,
    roff::{Roff, bold, roman},
};

use crate::cli::{Cli, CompletionShell};

pub fn completions(shell: CompletionShell, writer: &mut dyn Write) {
    let mut command = Cli::command();
    let shell = match shell {
        CompletionShell::Bash => Shell::Bash,
        CompletionShell::Zsh => Shell::Zsh,
        CompletionShell::Fish => Shell::Fish,
    };
    generate(shell, &mut command, "synesthesia", writer);
}

pub fn manpage(writer: &mut dyn Write) -> io::Result<()> {
    let command = Cli::command();
    let man = Man::new(command)
        .title("SYNESTHESIA")
        .section("1")
        .source(format!("synesthesia {}", env!("CARGO_PKG_VERSION")))
        .manual("Synesthesia Manual");

    man.render(writer)?;

    let mut policy = Roff::default();
    policy.control("SH", ["EXPERIMENTAL LINUX SOURCES"]);
    policy.text([roman(
        "Scheduler and TCP pathology modes use separate Linux eBPF collector helpers. \
         They require compatible x86_64 kernel BTF, tracepoints, and privilege supplied \
         explicitly by the user. Synesthesia never invokes sudo, installs capabilities, \
         or creates setuid files. Attachment does not prove that an event will occur.",
    )]);
    policy.control("SH", ["ENVIRONMENT"]);
    policy.text([bold("TERM"), roman(", "), bold("COLORTERM")]);
    policy.text([roman(
        "Conventional terminal capability hints. They are inferred, not actively probed.",
    )]);
    policy.text([bold("NO_COLOR")]);
    policy.text([roman(
        "When present, disables doctor status color and causes conservative renderer fallback.",
    )]);
    policy.control("SH", ["EXAMPLES"]);
    for example in [
        "synesthesia demo",
        "synesthesia proc",
        "synesthesia doctor --format json",
        "synesthesia replay incident.ndjson --speed 0.2",
        "sudo synesthesia ebpf scheduler",
    ] {
        policy.control("TP", ["4"]);
        policy.text([bold(example)]);
    }
    policy.control("SH", ["FILES"]);
    policy.text([roman(
        "Synesthesia has no required configuration file, database, daemon state, or persistent \
         BPF pin. Recording paths are supplied explicitly by the user.",
    )]);
    policy.control("SH", ["DIAGNOSTICS"]);
    policy.text([roman(
        "synesthesia doctor is passive by default and emits a public-safe text or schema-v1 JSON \
         report. --check-live is an explicitly active immediate attach/detach test and never \
         escalates privilege or generates workload.",
    )]);
    policy.control("SH", ["EXIT STATUS"]);
    policy.text([roman(
        "Doctor returns 0 when the report completes without a requested-check failure, 1 when an \
         explicitly requested check fails, and 2 when it cannot construct or write valid output. \
         Other commands return nonzero on source, input, recording, or terminal failure.",
    )]);
    policy.control("SH", ["SECURITY AND PRIVACY"]);
    policy.text([roman(
        "Doctor omits hostnames, usernames, home paths, process identities, network endpoints, \
         arguments, environment contents, and captured activity. Process recordings can contain \
         bounded comm names and PIDs unless --anonymize is used. TCP recordings can contain \
         endpoint metadata. No eBPF source captures payloads.",
    )]);
    policy.control("SH", ["PROJECT"]);
    policy.text([roman("https://github.com/unpingable/synesthesia")]);
    policy.control("SH", ["LICENSE"]);
    policy.text([roman("Apache License 2.0.")]);
    policy.control("SH", ["DESCRIPTION NOTE"]);
    policy.text([roman(
        "Wireshark is analysis. Synesthesia is the hallucination layer.",
    )]);
    policy.to_writer(writer)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_completion_scripts_are_nonempty_deterministic_and_cli_derived() {
        for shell in [
            CompletionShell::Bash,
            CompletionShell::Zsh,
            CompletionShell::Fish,
        ] {
            let mut first = Vec::new();
            let mut second = Vec::new();
            completions(shell, &mut first);
            completions(shell, &mut second);
            assert_eq!(first, second);
            assert!(first.len() > 500);
            let output = String::from_utf8(first).unwrap();
            for command in ["doctor", "proc", "ebpf", "replay", "demo"] {
                assert!(output.contains(command), "{shell:?} omitted {command}");
            }
            assert!(!output.contains('\u{1b}'));
            assert!(!output.contains("Data in. Terminal weather out."));
        }
    }

    #[test]
    fn manpage_is_deterministic_current_and_policy_complete() {
        let mut first = Vec::new();
        let mut second = Vec::new();
        manpage(&mut first).unwrap();
        manpage(&mut second).unwrap();
        assert_eq!(first, second);
        let output = String::from_utf8(first).unwrap();
        assert!(output.lines().any(|line| line.starts_with(".TH")));
        assert!(output.contains(env!("CARGO_PKG_VERSION")));
        for command in ["doctor", "proc", "ebpf", "replay", "demo"] {
            assert!(output.contains(command), "man page omitted {command}");
        }
        assert!(output.contains("never invokes sudo"));
        assert!(output.contains("Apache License 2.0"));
        assert!(!output.contains('\u{1b}'));
    }
}
