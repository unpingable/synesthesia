use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{self, IsTerminal},
    path::{Path, PathBuf},
};

use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    cli::DoctorArgs,
    source::ebpf_prerequisites::{
        SCHEDULER_BTF_TYPES, SCHEDULER_COLLECTOR, SCHEDULER_TRACEPOINTS, SUPPORTED_ARCHITECTURE,
        TCP_BTF_TYPES, TCP_COLLECTOR, TCP_TRACEPOINTS, TracepointSpec,
    },
};

pub const DOCTOR_SCHEMA_VERSION: u32 = 1;
pub const PRIVACY_NOTICE: &str = "This report does not include process arguments, environment contents, network endpoints, or captured activity.";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Unknown,
    NotApplicable,
    NotTested,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckGroup {
    Build,
    Terminal,
    Proc,
    Ebpf,
    Scheduler,
    Tcp,
}

impl CheckGroup {
    fn label(self) -> &'static str {
        match self {
            Self::Build => "Build and platform",
            Self::Terminal => "Terminal",
            Self::Proc => "/proc",
            Self::Ebpf => "eBPF prerequisites",
            Self::Scheduler => "Scheduler source",
            Self::Tcp => "TCP pathology source",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DoctorCheck {
    pub id: String,
    pub group: CheckGroup,
    pub label: String,
    pub status: CheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed: Option<Value>,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    pub safe_to_share: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlatformReport {
    pub os: String,
    pub architecture: String,
    pub kernel: String,
    pub libc: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeAvailability {
    Available,
    AvailableWithLimitations,
    UnsupportedPlatform,
    MissingPrerequisite,
    PermissionRequired,
    NotIncludedInBuild,
    Unknown,
}

impl ModeAvailability {
    fn text(self) -> &'static str {
        match self {
            Self::Available => "ready",
            Self::AvailableWithLimitations => "prerequisites present; live attachment not tested",
            Self::UnsupportedPlatform => "unsupported platform",
            Self::MissingPrerequisite => "missing prerequisite",
            Self::PermissionRequired => "prerequisites present; external privilege required",
            Self::NotIncludedInBuild => "not included in this build",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DoctorReport {
    pub schema_version: u32,
    pub synesthesia_version: String,
    pub git_commit: String,
    pub platform: PlatformReport,
    pub checks: Vec<DoctorCheck>,
    pub mode_summary: BTreeMap<String, ModeAvailability>,
    pub privacy_notice: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ColorLevel {
    Monochrome,
    Ansi16,
    Ansi256,
    Truecolor,
    Unknown,
}

impl ColorLevel {
    fn label(self) -> &'static str {
        match self {
            Self::Monochrome => "monochrome",
            Self::Ansi16 => "ANSI 16 (inferred)",
            Self::Ansi256 => "ANSI 256 (inferred)",
            Self::Truecolor => "truecolor (inferred)",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug)]
struct DiagnosticSnapshot {
    os: String,
    architecture: String,
    kernel: Option<String>,
    target_env: String,
    stdin_tty: bool,
    stdout_tty: bool,
    stderr_tty: bool,
    terminal_size: Option<(u16, u16)>,
    term: Option<String>,
    colorterm: Option<String>,
    no_color: bool,
    locale: Option<String>,
    proc_root: bool,
    proc_stat: bool,
    proc_meminfo: bool,
    proc_pids: ProbeResult,
    proc_self_stat: bool,
    proc_self_io: bool,
    proc_partial_visibility: ProbeResult,
    btf: bool,
    bpf_fs: bool,
    bpf_fs_mounted: Option<bool>,
    trace_visibility: ProbeResult,
    unprivileged_bpf: Option<String>,
    lockdown: Option<String>,
    effective_root: Option<bool>,
    effective_bpf_caps: Option<bool>,
    scheduler_collector: bool,
    tcp_collector: bool,
    scheduler_tracepoints: BTreeMap<String, ProbeResult>,
    tcp_tracepoints: BTreeMap<String, ProbeResult>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProbeResult {
    Available,
    Restricted,
    Missing,
    Unknown,
}

pub struct DoctorOutcome {
    pub output: String,
    pub exit_code: u8,
}

pub fn run(args: &DoctorArgs) -> Result<DoctorOutcome, serde_json::Error> {
    let snapshot = collect_snapshot();
    let mut report = build_report(&snapshot);
    let mut requested_failure = false;

    if args.check_live {
        requested_failure |= add_live_checks(&mut report);
    }
    if args.check_ebpf {
        requested_failure |= ["scheduler", "tcp"].iter().any(|mode| {
            !matches!(
                report.mode_summary.get(*mode),
                Some(
                    ModeAvailability::Available
                        | ModeAvailability::AvailableWithLimitations
                        | ModeAvailability::PermissionRequired
                )
            )
        });
    }

    let output = match args.format {
        crate::cli::DoctorFormat::Json => format!("{}\n", serde_json::to_string_pretty(&report)?),
        crate::cli::DoctorFormat::Text => render_text(
            &report,
            args.verbose,
            !args.no_color && snapshot.stdout_tty && !snapshot.no_color,
        ),
    };
    Ok(DoctorOutcome {
        output,
        exit_code: u8::from(requested_failure),
    })
}

fn collect_snapshot() -> DiagnosticSnapshot {
    let os = env::consts::OS.to_owned();
    let architecture = env::consts::ARCH.to_owned();
    let kernel = if os == "linux" {
        read_bounded(Path::new("/proc/sys/kernel/osrelease"), 128)
    } else {
        None
    };
    let (trace_root, trace_visibility) = inspect_tracepoint_root();
    let scheduler_tracepoints = tracepoint_presence(
        trace_root.as_deref(),
        trace_visibility,
        &SCHEDULER_TRACEPOINTS,
    );
    let tcp_tracepoints =
        tracepoint_presence(trace_root.as_deref(), trace_visibility, &TCP_TRACEPOINTS);
    let (proc_pids, proc_partial_visibility) = inspect_proc_visibility();
    let (effective_root, effective_bpf_caps) = inspect_identity();
    let executable_dir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));

    DiagnosticSnapshot {
        os,
        architecture,
        kernel,
        target_env: format!("{}/{}", env::consts::OS, target_environment()),
        stdin_tty: io::stdin().is_terminal(),
        stdout_tty: io::stdout().is_terminal(),
        stderr_tty: io::stderr().is_terminal(),
        terminal_size: io::stdout()
            .is_terminal()
            .then(crossterm::terminal::size)
            .and_then(Result::ok),
        term: safe_env("TERM"),
        colorterm: safe_env("COLORTERM"),
        no_color: env::var_os("NO_COLOR").is_some(),
        locale: safe_env("LC_ALL")
            .or_else(|| safe_env("LC_CTYPE"))
            .or_else(|| safe_env("LANG")),
        proc_root: Path::new("/proc").is_dir(),
        proc_stat: readable_file(Path::new("/proc/stat")),
        proc_meminfo: readable_file(Path::new("/proc/meminfo")),
        proc_pids,
        proc_self_stat: readable_file(Path::new("/proc/self/stat")),
        proc_self_io: readable_file(Path::new("/proc/self/io")),
        proc_partial_visibility,
        btf: readable_file(Path::new("/sys/kernel/btf/vmlinux")),
        bpf_fs: Path::new("/sys/fs/bpf").is_dir(),
        bpf_fs_mounted: detect_bpf_mount(),
        trace_visibility,
        unprivileged_bpf: read_bounded(Path::new("/proc/sys/kernel/unprivileged_bpf_disabled"), 32),
        lockdown: read_bounded(Path::new("/sys/kernel/security/lockdown"), 128)
            .map(|value| selected_lockdown(&value)),
        effective_root,
        effective_bpf_caps,
        scheduler_collector: executable_dir
            .as_deref()
            .is_some_and(|dir| is_executable(&dir.join(SCHEDULER_COLLECTOR))),
        tcp_collector: executable_dir
            .as_deref()
            .is_some_and(|dir| is_executable(&dir.join(TCP_COLLECTOR))),
        scheduler_tracepoints,
        tcp_tracepoints,
    }
}

fn build_report(snapshot: &DiagnosticSnapshot) -> DoctorReport {
    let mut checks = Vec::new();
    let linux = snapshot.os == "linux";
    let supported_architecture = snapshot.architecture == SUPPORTED_ARCHITECTURE;
    let ebpf_build = cfg!(feature = "ebpf");
    let color = infer_color(
        snapshot.term.as_deref(),
        snapshot.colorterm.as_deref(),
        snapshot.no_color,
    );
    let unicode = infer_unicode(snapshot.locale.as_deref());

    push(
        &mut checks,
        "build.version",
        CheckGroup::Build,
        "Synesthesia version",
        CheckStatus::Pass,
        Some(json!(env!("CARGO_PKG_VERSION"))),
        "version embedded in this executable",
        None,
    );
    push(
        &mut checks,
        "build.git_commit",
        CheckGroup::Build,
        "Build commit",
        if option_env!("SYNESTHESIA_GIT_COMMIT").is_some() {
            CheckStatus::Pass
        } else {
            CheckStatus::Unknown
        },
        option_env!("SYNESTHESIA_GIT_COMMIT").map(|value| json!(value)),
        if option_env!("SYNESTHESIA_GIT_COMMIT").is_some() {
            "reproducibly embedded by the build"
        } else {
            "commit identity was not available to this build"
        },
        None,
    );
    push(
        &mut checks,
        "platform.target",
        CheckGroup::Build,
        "Target platform",
        CheckStatus::Pass,
        Some(json!(format!("{}/{}", snapshot.os, snapshot.architecture))),
        "compile-time Rust target",
        None,
    );
    push(
        &mut checks,
        "platform.kernel",
        CheckGroup::Build,
        "Kernel release",
        if linux && snapshot.kernel.is_some() {
            CheckStatus::Pass
        } else if linux {
            CheckStatus::Unknown
        } else {
            CheckStatus::NotApplicable
        },
        snapshot.kernel.as_ref().map(|value| json!(value)),
        if linux {
            "read from the kernel release interface"
        } else {
            "Linux kernel checks do not apply"
        },
        None,
    );
    let libc = libc_description(&snapshot.target_env);
    push(
        &mut checks,
        "platform.libc",
        CheckGroup::Build,
        "C runtime",
        CheckStatus::Unknown,
        Some(json!(libc)),
        "target family is known; runtime library version is not actively probed",
        None,
    );
    push(
        &mut checks,
        "build.ebpf",
        CheckGroup::Build,
        "eBPF support in build",
        if ebpf_build {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        Some(json!(ebpf_build)),
        if ebpf_build {
            "the launcher contains the Linux eBPF loader"
        } else {
            "this launcher was built without the ebpf feature"
        },
        Some("Build release binaries with `cargo build --release --features ebpf --bins`."),
    );

    for (id, label, value) in [
        ("terminal.stdin_tty", "stdin is a TTY", snapshot.stdin_tty),
        (
            "terminal.stdout_tty",
            "stdout is a TTY",
            snapshot.stdout_tty,
        ),
        (
            "terminal.stderr_tty",
            "stderr is a TTY",
            snapshot.stderr_tty,
        ),
    ] {
        push(
            &mut checks,
            id,
            CheckGroup::Terminal,
            label,
            if value {
                CheckStatus::Pass
            } else {
                CheckStatus::Warn
            },
            Some(json!(value)),
            if value {
                "attached to a terminal"
            } else {
                "not attached; interactive mode may be unsuitable, but snapshots remain usable"
            },
            None,
        );
    }
    push(
        &mut checks,
        "terminal.size",
        CheckGroup::Terminal,
        "Terminal dimensions",
        snapshot
            .terminal_size
            .map_or(CheckStatus::Unknown, |_| CheckStatus::Pass),
        snapshot
            .terminal_size
            .map(|(width, height)| json!({"width": width, "height": height})),
        "queried without emitting terminal control sequences",
        None,
    );
    push(
        &mut checks,
        "terminal.environment",
        CheckGroup::Terminal,
        "Terminal environment",
        if snapshot.term.is_some() {
            CheckStatus::Pass
        } else {
            CheckStatus::Unknown
        },
        Some(json!({
            "TERM": snapshot.term,
            "COLORTERM": snapshot.colorterm,
            "NO_COLOR": snapshot.no_color
        })),
        "capability is inferred from conventional environment hints, not proven",
        None,
    );
    push(
        &mut checks,
        "terminal.color",
        CheckGroup::Terminal,
        "Likely color level",
        if color == ColorLevel::Unknown {
            CheckStatus::Unknown
        } else {
            CheckStatus::Pass
        },
        Some(json!(color.label())),
        "inferred without emitting a color probe",
        None,
    );
    push(
        &mut checks,
        "terminal.unicode",
        CheckGroup::Terminal,
        "Unicode locale likelihood",
        match unicode {
            Some(true) => CheckStatus::Pass,
            Some(false) => CheckStatus::Warn,
            None => CheckStatus::Unknown,
        },
        unicode.map(|value| json!(value)),
        "inferred from locale naming only; ASCII mode remains available",
        None,
    );
    push(
        &mut checks,
        "terminal.interactive",
        CheckGroup::Terminal,
        "Interactive rendering",
        if snapshot.stdin_tty && snapshot.stdout_tty {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        Some(json!(snapshot.stdin_tty && snapshot.stdout_tty)),
        if snapshot.stdin_tty && snapshot.stdout_tty {
            "raw input and alternate-screen rendering are plausible"
        } else {
            "use `--snapshot` when input or output is redirected"
        },
        None,
    );

    add_proc_checks(&mut checks, snapshot, linux);
    add_ebpf_checks(
        &mut checks,
        snapshot,
        linux,
        supported_architecture,
        ebpf_build,
    );

    let proc_ready = linux
        && snapshot.proc_stat
        && snapshot.proc_meminfo
        && snapshot.proc_self_stat
        && snapshot.proc_pids == ProbeResult::Available;
    let scheduler = ebpf_mode(
        linux,
        supported_architecture,
        ebpf_build,
        snapshot.btf,
        snapshot.scheduler_collector,
        snapshot
            .scheduler_tracepoints
            .values()
            .all(|result| *result == ProbeResult::Available),
        snapshot
            .scheduler_tracepoints
            .values()
            .any(|result| *result == ProbeResult::Restricted),
        privilege_likely(snapshot),
    );
    let tcp = ebpf_mode(
        linux,
        supported_architecture,
        ebpf_build,
        snapshot.btf,
        snapshot.tcp_collector,
        snapshot
            .tcp_tracepoints
            .values()
            .all(|result| *result == ProbeResult::Available),
        snapshot
            .tcp_tracepoints
            .values()
            .any(|result| *result == ProbeResult::Restricted),
        privilege_likely(snapshot),
    );
    let mode_summary = BTreeMap::from([
        ("demo".to_owned(), ModeAvailability::Available),
        (
            "proc".to_owned(),
            if !linux {
                ModeAvailability::UnsupportedPlatform
            } else if proc_ready {
                ModeAvailability::Available
            } else {
                ModeAvailability::MissingPrerequisite
            },
        ),
        ("replay".to_owned(), ModeAvailability::Available),
        ("scheduler".to_owned(), scheduler),
        ("schema".to_owned(), ModeAvailability::Available),
        ("stdin".to_owned(), ModeAvailability::Available),
        ("tcp".to_owned(), tcp),
    ]);

    DoctorReport {
        schema_version: DOCTOR_SCHEMA_VERSION,
        synesthesia_version: env!("CARGO_PKG_VERSION").to_owned(),
        git_commit: option_env!("SYNESTHESIA_GIT_COMMIT")
            .unwrap_or("unknown")
            .to_owned(),
        platform: PlatformReport {
            os: snapshot.os.clone(),
            architecture: snapshot.architecture.clone(),
            kernel: snapshot
                .kernel
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
            libc,
        },
        checks,
        mode_summary,
        privacy_notice: PRIVACY_NOTICE.to_owned(),
    }
}

fn add_proc_checks(checks: &mut Vec<DoctorCheck>, snapshot: &DiagnosticSnapshot, linux: bool) {
    if !linux {
        push(
            checks,
            "proc.platform",
            CheckGroup::Proc,
            "Linux procfs",
            CheckStatus::NotApplicable,
            None,
            "process weather is Linux-only",
            None,
        );
        return;
    }
    for (id, label, available) in [
        ("proc.root", "/proc directory", snapshot.proc_root),
        ("proc.stat", "/proc/stat", snapshot.proc_stat),
        ("proc.meminfo", "/proc/meminfo", snapshot.proc_meminfo),
        (
            "proc.self_stat",
            "current process stat",
            snapshot.proc_self_stat,
        ),
        (
            "proc.self_io",
            "current process I/O counters",
            snapshot.proc_self_io,
        ),
    ] {
        push(
            checks,
            id,
            CheckGroup::Proc,
            label,
            if available {
                CheckStatus::Pass
            } else {
                CheckStatus::Warn
            },
            Some(json!(available)),
            if available {
                "readable"
            } else {
                "missing or unreadable; process weather may be partial"
            },
            None,
        );
    }
    add_probe_result(
        checks,
        "proc.enumeration",
        CheckGroup::Proc,
        "Numeric PID enumeration",
        snapshot.proc_pids,
        "numeric process directories can be enumerated without printing identities",
    );
    add_probe_result(
        checks,
        "proc.visibility",
        CheckGroup::Proc,
        "Process visibility",
        snapshot.proc_partial_visibility,
        "bounded permission sampling found no hidepid-style restriction",
    );
}

fn add_ebpf_checks(
    checks: &mut Vec<DoctorCheck>,
    snapshot: &DiagnosticSnapshot,
    linux: bool,
    supported_architecture: bool,
    ebpf_build: bool,
) {
    push(
        checks,
        "ebpf.platform",
        CheckGroup::Ebpf,
        "Linux platform",
        if linux {
            CheckStatus::Pass
        } else {
            CheckStatus::NotApplicable
        },
        Some(json!(linux)),
        if linux {
            "Linux eBPF interfaces may be inspected"
        } else {
            "eBPF sources are Linux-only"
        },
        None,
    );
    push(
        checks,
        "ebpf.architecture",
        CheckGroup::Ebpf,
        "Supported architecture",
        if !linux {
            CheckStatus::NotApplicable
        } else if supported_architecture {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        Some(json!(&snapshot.architecture)),
        "current experimental collectors support x86_64 only",
        None,
    );
    push(
        checks,
        "ebpf.btf",
        CheckGroup::Ebpf,
        "Kernel BTF",
        if !linux {
            CheckStatus::NotApplicable
        } else if snapshot.btf {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        Some(json!(snapshot.btf)),
        if snapshot.btf {
            "/sys/kernel/btf/vmlinux is readable"
        } else {
            "required kernel BTF is absent or unreadable"
        },
        None,
    );
    push(
        checks,
        "ebpf.bpffs",
        CheckGroup::Ebpf,
        "BPF filesystem",
        if !linux {
            CheckStatus::NotApplicable
        } else if snapshot.bpf_fs {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        Some(json!({"directory": snapshot.bpf_fs, "mounted": snapshot.bpf_fs_mounted})),
        "presence is passive information; Synesthesia does not pin objects",
        None,
    );
    push(
        checks,
        "ebpf.tracefs",
        CheckGroup::Ebpf,
        "Tracepoint filesystem",
        if !linux {
            CheckStatus::NotApplicable
        } else {
            match snapshot.trace_visibility {
                ProbeResult::Available => CheckStatus::Pass,
                ProbeResult::Restricted | ProbeResult::Missing => CheckStatus::Warn,
                ProbeResult::Unknown => CheckStatus::Unknown,
            }
        },
        Some(json!(match snapshot.trace_visibility {
            ProbeResult::Available => "available",
            ProbeResult::Restricted => "restricted",
            ProbeResult::Missing => "missing",
            ProbeResult::Unknown => "unknown",
        })),
        if snapshot.trace_visibility == ProbeResult::Restricted {
            "tracepoint root exists but is inaccessible to the current identity; kernel support is not disproven"
        } else {
            "checked the standard tracefs and debugfs tracepoint roots"
        },
        None,
    );
    let policy_status = match snapshot.unprivileged_bpf.as_deref() {
        Some("0") => CheckStatus::Pass,
        Some(_) => CheckStatus::Warn,
        None => CheckStatus::Unknown,
    };
    push(
        checks,
        "ebpf.unprivileged_policy",
        CheckGroup::Ebpf,
        "Unprivileged BPF policy",
        if linux {
            policy_status
        } else {
            CheckStatus::NotApplicable
        },
        snapshot.unprivileged_bpf.as_ref().map(|value| json!(value)),
        match snapshot.unprivileged_bpf.as_deref() {
            Some("0") => {
                "kernel policy permits unprivileged BPF requests; other controls still apply"
            }
            Some(_) => {
                "kernel policy restricts unprivileged BPF; external privilege is likely required"
            }
            None => "policy was not readable",
        },
        None,
    );
    push(
        checks,
        "ebpf.lockdown",
        CheckGroup::Ebpf,
        "Kernel lockdown",
        match snapshot.lockdown.as_deref() {
            Some("none") => CheckStatus::Pass,
            Some(_) => CheckStatus::Warn,
            None => CheckStatus::Unknown,
        },
        snapshot.lockdown.as_ref().map(|value| json!(value)),
        "lockdown can restrict BPF even when other prerequisites are present",
        None,
    );
    let ring = snapshot
        .kernel
        .as_deref()
        .and_then(kernel_likely_has_ring_buffer);
    push(
        checks,
        "ebpf.ring_buffer",
        CheckGroup::Ebpf,
        "Ring-buffer support",
        match ring {
            Some(true) => CheckStatus::Pass,
            Some(false) => CheckStatus::Warn,
            None => CheckStatus::Unknown,
        },
        ring.map(|value| json!(value)),
        "inferred from kernel release (5.8 or newer); only attachment can prove availability",
        None,
    );
    push(
        checks,
        "ebpf.identity",
        CheckGroup::Ebpf,
        "Likely BPF privilege",
        match privilege_likely(snapshot) {
            Some(true) => CheckStatus::Pass,
            Some(false) => CheckStatus::Warn,
            None => CheckStatus::Unknown,
        },
        Some(json!({
            "effective_root": snapshot.effective_root,
            "relevant_capabilities": snapshot.effective_bpf_caps
        })),
        "root or relevant effective capabilities may permit attachment; verifier acceptance is never guaranteed",
        Some(
            "Synesthesia never escalates. Supply suitable privilege externally if you choose to run an eBPF mode.",
        ),
    );

    add_source_checks(
        checks,
        SourceDiagnostic {
            group: CheckGroup::Scheduler,
            prefix: "scheduler",
            label: "Scheduler",
            build: ebpf_build,
            linux,
            supported_architecture,
            btf: snapshot.btf,
            collector: snapshot.scheduler_collector,
            collector_name: SCHEDULER_COLLECTOR,
            tracepoints: &snapshot.scheduler_tracepoints,
            btf_types: &SCHEDULER_BTF_TYPES,
        },
    );
    add_source_checks(
        checks,
        SourceDiagnostic {
            group: CheckGroup::Tcp,
            prefix: "tcp",
            label: "TCP",
            build: ebpf_build,
            linux,
            supported_architecture,
            btf: snapshot.btf,
            collector: snapshot.tcp_collector,
            collector_name: TCP_COLLECTOR,
            tracepoints: &snapshot.tcp_tracepoints,
            btf_types: &TCP_BTF_TYPES,
        },
    );
}

struct SourceDiagnostic<'a> {
    group: CheckGroup,
    prefix: &'static str,
    label: &'static str,
    build: bool,
    linux: bool,
    supported_architecture: bool,
    btf: bool,
    collector: bool,
    collector_name: &'static str,
    tracepoints: &'a BTreeMap<String, ProbeResult>,
    btf_types: &'a [&'static str],
}

fn add_source_checks(checks: &mut Vec<DoctorCheck>, source: SourceDiagnostic<'_>) {
    push(
        checks,
        &format!("{}.collector", source.prefix),
        source.group,
        &format!("{} collector", source.label),
        if !source.linux {
            CheckStatus::NotApplicable
        } else if source.collector {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        Some(json!({"name": source.collector_name, "executable": source.collector})),
        if source.collector {
            "expected sibling collector is executable"
        } else {
            "expected sibling collector is absent or not executable"
        },
        None,
    );
    for (name, result) in source.tracepoints {
        let (status, observed, summary) = match result {
            ProbeResult::Available => (
                CheckStatus::Pass,
                "available",
                "tracepoint directory is present; attachment was not tested",
            ),
            ProbeResult::Restricted => (
                CheckStatus::Unknown,
                "restricted",
                "tracepoint root is inaccessible; presence and attachment remain untested",
            ),
            ProbeResult::Missing => (
                CheckStatus::Warn,
                "missing",
                "tracepoint directory is absent",
            ),
            ProbeResult::Unknown => (
                CheckStatus::Unknown,
                "unknown",
                "tracepoint presence could not be determined",
            ),
        };
        push(
            checks,
            &format!("{}.tracepoint.{}", source.prefix, name),
            source.group,
            &format!("tracepoint {name}"),
            if !source.linux {
                CheckStatus::NotApplicable
            } else {
                status
            },
            Some(json!(observed)),
            summary,
            None,
        );
    }
    push(
        checks,
        &format!("{}.btf_layout", source.prefix),
        source.group,
        &format!("{} BTF layouts", source.label),
        if !source.linux || !source.supported_architecture || !source.build || !source.btf {
            CheckStatus::NotApplicable
        } else {
            CheckStatus::NotTested
        },
        Some(json!(source.btf_types)),
        "the required CO-RE type layouts are validated only by an explicitly requested live attachment",
        None,
    );
    push(
        checks,
        &format!("{}.live_attachment", source.prefix),
        source.group,
        &format!("{} live attachment", source.label),
        CheckStatus::NotTested,
        None,
        "passive doctor never attaches probes",
        Some("Use `doctor --check-live` only when an immediate attach/detach test is intended."),
    );
}

fn add_live_checks(report: &mut DoctorReport) -> bool {
    let mut failed = false;
    let scheduler = live_scheduler_check();
    failed |= scheduler.status != CheckStatus::Pass;
    report.checks.push(scheduler);
    let tcp = live_tcp_check();
    failed |= tcp.status != CheckStatus::Pass;
    report.checks.push(tcp);
    failed
}

#[cfg(all(target_os = "linux", feature = "ebpf"))]
fn live_scheduler_check() -> DoctorCheck {
    match crate::source::scheduler_live::LiveScheduler::attach() {
        Ok(attached) => {
            drop(attached);
            live_check(
                "scheduler",
                CheckStatus::Pass,
                "attached and detached immediately",
            )
        }
        Err(error) => live_check("scheduler", CheckStatus::Fail, &error.to_string()),
    }
}

#[cfg(not(all(target_os = "linux", feature = "ebpf")))]
fn live_scheduler_check() -> DoctorCheck {
    live_check(
        "scheduler",
        CheckStatus::Fail,
        "live scheduler checking is not included in this build",
    )
}

#[cfg(all(target_os = "linux", feature = "ebpf"))]
fn live_tcp_check() -> DoctorCheck {
    match crate::source::tcp_live::LiveTcp::attach() {
        Ok(attached) => {
            drop(attached);
            live_check(
                "tcp",
                CheckStatus::Pass,
                "attached and detached immediately",
            )
        }
        Err(error) => live_check("tcp", CheckStatus::Fail, &error.to_string()),
    }
}

#[cfg(not(all(target_os = "linux", feature = "ebpf")))]
fn live_tcp_check() -> DoctorCheck {
    live_check(
        "tcp",
        CheckStatus::Fail,
        "live TCP checking is not included in this build",
    )
}

fn live_check(source: &str, status: CheckStatus, summary: &str) -> DoctorCheck {
    DoctorCheck {
        id: format!("{source}.live_attachment_active"),
        group: if source == "scheduler" {
            CheckGroup::Scheduler
        } else {
            CheckGroup::Tcp
        },
        label: format!("{source} active attach/detach test"),
        status,
        observed: Some(json!("explicitly requested active test")),
        summary: summary.to_owned(),
        remediation: None,
        safe_to_share: true,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the pure readiness decision keeps each independent prerequisite explicit"
)]
fn ebpf_mode(
    linux: bool,
    supported_architecture: bool,
    build: bool,
    btf: bool,
    collector: bool,
    tracepoints: bool,
    tracepoints_restricted: bool,
    privilege: Option<bool>,
) -> ModeAvailability {
    if !linux {
        ModeAvailability::UnsupportedPlatform
    } else if !build {
        ModeAvailability::NotIncludedInBuild
    } else if !supported_architecture || !btf || !collector {
        ModeAvailability::MissingPrerequisite
    } else if tracepoints_restricted && privilege == Some(false) {
        ModeAvailability::PermissionRequired
    } else if tracepoints_restricted {
        ModeAvailability::Unknown
    } else if !tracepoints {
        ModeAvailability::MissingPrerequisite
    } else if privilege == Some(false) {
        ModeAvailability::PermissionRequired
    } else {
        ModeAvailability::AvailableWithLimitations
    }
}

fn privilege_likely(snapshot: &DiagnosticSnapshot) -> Option<bool> {
    match (snapshot.effective_root, snapshot.effective_bpf_caps) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

fn render_text(report: &DoctorReport, verbose: bool, color: bool) -> String {
    let mut output = format!(
        "Synesthesia doctor {}  schema {}\nPlatform: {} {}  kernel {}  {}\n\n",
        report.synesthesia_version,
        report.schema_version,
        report.platform.os,
        report.platform.architecture,
        report.platform.kernel,
        report.platform.libc
    );
    let mut previous = None;
    for check in &report.checks {
        if previous != Some(check.group) {
            previous = Some(check.group);
            output.push_str(check.group.label());
            output.push('\n');
        }
        let status = status_text(check.status, color);
        output.push_str(&format!(
            "  {status:<12} {:<28} {}\n",
            check.label, check.summary
        ));
        if verbose {
            output.push_str(&format!("               id: {}\n", check.id));
            if let Some(observed) = &check.observed {
                output.push_str(&format!(
                    "               observed: {}\n",
                    compact_json(observed)
                ));
            }
            if let Some(remediation) = &check.remediation {
                output.push_str(&format!("               hint: {remediation}\n"));
            }
        }
    }
    output.push_str("\nModes\n");
    for name in [
        "demo",
        "stdin",
        "schema",
        "replay",
        "proc",
        "scheduler",
        "tcp",
    ] {
        if let Some(status) = report.mode_summary.get(name) {
            output.push_str(&format!("  {name:<10} {}\n", status.text()));
        }
    }
    output.push_str("\nTry:\n");
    let scheduler = report.mode_summary.get("scheduler");
    let tcp = report.mode_summary.get("tcp");
    if scheduler == Some(&ModeAvailability::PermissionRequired) {
        output.push_str("  sudo synesthesia ebpf scheduler\n");
        output.push_str("  (Synesthesia will not escalate privileges itself.)\n");
    } else if tcp == Some(&ModeAvailability::PermissionRequired) {
        output.push_str("  sudo synesthesia ebpf tcp\n");
        output.push_str("  (Synesthesia will not escalate privileges itself.)\n");
    } else {
        output.push_str("  synesthesia demo\n");
    }
    output.push('\n');
    output.push_str(&report.privacy_notice);
    output.push('\n');
    output
}

fn status_text(status: CheckStatus, color: bool) -> String {
    let label = match status {
        CheckStatus::Pass => "PASS",
        CheckStatus::Warn => "WARN",
        CheckStatus::Fail => "FAIL",
        CheckStatus::Unknown => "UNKNOWN",
        CheckStatus::NotApplicable => "N/A",
        CheckStatus::NotTested => "NOT TESTED",
    };
    if !color {
        return label.to_owned();
    }
    let code = match status {
        CheckStatus::Pass => "32",
        CheckStatus::Warn | CheckStatus::Unknown | CheckStatus::NotTested => "33",
        CheckStatus::Fail => "31",
        CheckStatus::NotApplicable => "2",
    };
    format!("\x1b[{code}m{label}\x1b[0m")
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}

#[expect(
    clippy::too_many_arguments,
    reason = "typed check construction keeps every public report field explicit at call sites"
)]
fn push(
    checks: &mut Vec<DoctorCheck>,
    id: &str,
    group: CheckGroup,
    label: &str,
    status: CheckStatus,
    observed: Option<Value>,
    summary: &str,
    remediation: Option<&str>,
) {
    checks.push(DoctorCheck {
        id: id.to_owned(),
        group,
        label: label.to_owned(),
        status,
        observed,
        summary: summary.to_owned(),
        remediation: remediation.map(str::to_owned),
        safe_to_share: true,
    });
}

fn add_probe_result(
    checks: &mut Vec<DoctorCheck>,
    id: &str,
    group: CheckGroup,
    label: &str,
    result: ProbeResult,
    pass_summary: &str,
) {
    let (status, summary) = match result {
        ProbeResult::Available => (CheckStatus::Pass, pass_summary),
        ProbeResult::Restricted => (CheckStatus::Warn, "visibility is restricted or partial"),
        ProbeResult::Missing => (CheckStatus::Warn, "interface is missing"),
        ProbeResult::Unknown => (CheckStatus::Unknown, "condition could not be determined"),
    };
    push(
        checks,
        id,
        group,
        label,
        status,
        Some(json!(match result {
            ProbeResult::Available => "available",
            ProbeResult::Restricted => "restricted",
            ProbeResult::Missing => "missing",
            ProbeResult::Unknown => "unknown",
        })),
        summary,
        None,
    );
}

fn infer_color(term: Option<&str>, colorterm: Option<&str>, no_color: bool) -> ColorLevel {
    if no_color || term == Some("dumb") {
        return ColorLevel::Monochrome;
    }
    if colorterm.is_some_and(|value| {
        let value = value.to_ascii_lowercase();
        value.contains("truecolor") || value.contains("24bit")
    }) {
        return ColorLevel::Truecolor;
    }
    if term.is_some_and(|value| value.contains("256color")) {
        return ColorLevel::Ansi256;
    }
    if term.is_some() {
        return ColorLevel::Ansi16;
    }
    ColorLevel::Unknown
}

fn infer_unicode(locale: Option<&str>) -> Option<bool> {
    locale.map(|value| {
        let normalized = value.to_ascii_lowercase();
        normalized.contains("utf-8") || normalized.contains("utf8")
    })
}

fn safe_env(name: &str) -> Option<String> {
    env::var(name).ok().map(|value| sanitize_value(&value, 128))
}

fn sanitize_value(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect()
}

fn read_bounded(path: &Path, limit: usize) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() > limit {
        return None;
    }
    Some(sanitize_value(
        String::from_utf8_lossy(&bytes).trim(),
        limit,
    ))
}

fn readable_file(path: &Path) -> bool {
    File::open(path).is_ok()
}

fn inspect_proc_visibility() -> (ProbeResult, ProbeResult) {
    let Ok(entries) = fs::read_dir("/proc") else {
        return if Path::new("/proc").exists() {
            (ProbeResult::Restricted, ProbeResult::Unknown)
        } else {
            (ProbeResult::Missing, ProbeResult::Missing)
        };
    };
    let mut numeric = 0_u32;
    let mut restricted = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.parse::<u32>().is_err() {
            continue;
        }
        numeric += 1;
        if numeric <= 64 {
            let stat = entry.path().join("stat");
            if let Err(error) = File::open(stat) {
                if error.kind() == io::ErrorKind::PermissionDenied {
                    restricted = true;
                }
            }
        }
    }
    if numeric == 0 {
        (ProbeResult::Unknown, ProbeResult::Unknown)
    } else if restricted {
        (ProbeResult::Available, ProbeResult::Restricted)
    } else {
        (ProbeResult::Available, ProbeResult::Available)
    }
}

fn inspect_identity() -> (Option<bool>, Option<bool>) {
    let Some(status) = read_bounded_multiline(Path::new("/proc/self/status"), 64 * 1024) else {
        return (None, None);
    };
    let effective_uid = status.lines().find_map(|line| {
        line.strip_prefix("Uid:")
            .and_then(|fields| fields.split_whitespace().nth(1))
            .and_then(|field| field.parse::<u32>().ok())
    });
    let cap_eff = status.lines().find_map(|line| {
        line.strip_prefix("CapEff:")
            .and_then(|field| u64::from_str_radix(field.trim(), 16).ok())
    });
    let relevant = cap_eff.map(|caps| {
        let sys_admin = caps & (1_u64 << 21) != 0;
        let perfmon = caps & (1_u64 << 38) != 0;
        let bpf = caps & (1_u64 << 39) != 0;
        sys_admin || (perfmon && bpf)
    });
    (effective_uid.map(|uid| uid == 0), relevant)
}

fn inspect_tracepoint_root() -> (Option<PathBuf>, ProbeResult) {
    let mut missing_root = None;
    for path in [
        PathBuf::from("/sys/kernel/tracing/events"),
        PathBuf::from("/sys/kernel/debug/tracing/events"),
    ] {
        match path.metadata() {
            Ok(metadata) if metadata.is_dir() => return (Some(path), ProbeResult::Available),
            Ok(_) => missing_root = Some(path),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                return (Some(path), ProbeResult::Restricted);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing_root = Some(path);
            }
            Err(_) => return (Some(path), ProbeResult::Unknown),
        }
    }
    (missing_root, ProbeResult::Missing)
}

fn tracepoint_presence(
    root: Option<&Path>,
    root_result: ProbeResult,
    specs: &[TracepointSpec],
) -> BTreeMap<String, ProbeResult> {
    specs
        .iter()
        .map(|spec| {
            let result = match (root, root_result) {
                (_, ProbeResult::Restricted) => ProbeResult::Restricted,
                (_, ProbeResult::Missing) => ProbeResult::Missing,
                (_, ProbeResult::Unknown) | (None, _) => ProbeResult::Unknown,
                (Some(root), ProbeResult::Available) => {
                    match root.join(spec.group).join(spec.name).metadata() {
                        Ok(metadata) if metadata.is_dir() => ProbeResult::Available,
                        Ok(_) => ProbeResult::Missing,
                        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                            ProbeResult::Restricted
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {
                            ProbeResult::Missing
                        }
                        Err(_) => ProbeResult::Unknown,
                    }
                }
            };
            (spec.name.to_owned(), result)
        })
        .collect()
}

fn detect_bpf_mount() -> Option<bool> {
    let content = read_bounded_multiline(Path::new("/proc/self/mountinfo"), 1024 * 1024)?;
    Some(content.lines().any(|line| {
        line.split_once(" - ")
            .is_some_and(|(_, tail)| tail.starts_with("bpf "))
    }))
}

fn read_bounded_multiline(path: &Path, limit: usize) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() > limit {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn selected_lockdown(value: &str) -> String {
    value
        .split_whitespace()
        .find_map(|field| field.strip_prefix('[')?.strip_suffix(']'))
        .unwrap_or("unknown")
        .to_owned()
}

fn kernel_likely_has_ring_buffer(release: &str) -> Option<bool> {
    let mut parts = release.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts
        .next()?
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse::<u32>()
        .ok()?;
    Some((major, minor) >= (5, 8))
}

fn libc_description(target: &str) -> String {
    if target.ends_with("/gnu") {
        "glibc-compatible target; runtime version not probed".to_owned()
    } else if target.ends_with("/musl") {
        "musl target; runtime version not probed".to_owned()
    } else {
        "unknown C runtime".to_owned()
    }
}

fn target_environment() -> &'static str {
    if cfg!(target_env = "gnu") {
        "gnu"
    } else if cfg!(target_env = "musl") {
        "musl"
    } else if cfg!(target_env = "msvc") {
        "msvc"
    } else {
        "unknown"
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake() -> DiagnosticSnapshot {
        DiagnosticSnapshot {
            os: "linux".to_owned(),
            architecture: "x86_64".to_owned(),
            kernel: Some("6.8.0-test".to_owned()),
            target_env: "linux/gnu".to_owned(),
            stdin_tty: true,
            stdout_tty: true,
            stderr_tty: true,
            terminal_size: Some((100, 30)),
            term: Some("xterm-256color".to_owned()),
            colorterm: Some("truecolor".to_owned()),
            no_color: false,
            locale: Some("C.UTF-8".to_owned()),
            proc_root: true,
            proc_stat: true,
            proc_meminfo: true,
            proc_pids: ProbeResult::Available,
            proc_self_stat: true,
            proc_self_io: true,
            proc_partial_visibility: ProbeResult::Available,
            btf: true,
            bpf_fs: true,
            bpf_fs_mounted: Some(true),
            trace_visibility: ProbeResult::Available,
            unprivileged_bpf: Some("2".to_owned()),
            lockdown: Some("none".to_owned()),
            effective_root: Some(false),
            effective_bpf_caps: Some(false),
            scheduler_collector: true,
            tcp_collector: true,
            scheduler_tracepoints: SCHEDULER_TRACEPOINTS
                .iter()
                .map(|spec| (spec.name.to_owned(), ProbeResult::Available))
                .collect(),
            tcp_tracepoints: TCP_TRACEPOINTS
                .iter()
                .map(|spec| (spec.name.to_owned(), ProbeResult::Available))
                .collect(),
        }
    }

    #[test]
    fn schema_and_status_names_are_stable() {
        assert_eq!(DOCTOR_SCHEMA_VERSION, 1);
        for (status, expected) in [
            (CheckStatus::Pass, "\"pass\""),
            (CheckStatus::Warn, "\"warn\""),
            (CheckStatus::Fail, "\"fail\""),
            (CheckStatus::Unknown, "\"unknown\""),
            (CheckStatus::NotApplicable, "\"not_applicable\""),
            (CheckStatus::NotTested, "\"not_tested\""),
        ] {
            assert_eq!(serde_json::to_string(&status).unwrap(), expected);
        }
        let report = build_report(&fake());
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert!(value["mode_summary"]["scheduler"].is_string());
        assert!(value["checks"][0]["safe_to_share"].is_boolean());
    }

    #[test]
    fn color_inference_is_conservative() {
        assert_eq!(
            infer_color(Some("xterm"), Some("truecolor"), false),
            ColorLevel::Truecolor
        );
        assert_eq!(
            infer_color(Some("xterm-256color"), None, false),
            ColorLevel::Ansi256
        );
        assert_eq!(infer_color(Some("xterm"), None, false), ColorLevel::Ansi16);
        assert_eq!(
            infer_color(Some("xterm"), Some("truecolor"), true),
            ColorLevel::Monochrome
        );
        assert_eq!(infer_color(None, None, false), ColorLevel::Unknown);
        assert_eq!(infer_unicode(Some("C")), Some(false));
        assert_eq!(infer_unicode(Some("en_US.UTF-8")), Some(true));
    }

    #[test]
    fn redirected_text_and_json_have_no_escapes_or_private_material() {
        let mut snapshot = fake();
        snapshot.stdout_tty = false;
        let report = build_report(&snapshot);
        let text = render_text(&report, false, false);
        let json = serde_json::to_string_pretty(&report).unwrap();
        for output in [&text, &json] {
            assert!(!output.contains('\u{1b}'));
            assert!(!output.contains("/home/"));
            assert!(!output.contains("10.0."));
            assert!(!output.contains("worker"));
            assert!(!output.contains("sushi-k"));
        }
        assert!(text.contains(PRIVACY_NOTICE));
    }

    #[test]
    fn fake_report_is_deterministic_and_distinguishes_privilege() {
        let first = serde_json::to_vec_pretty(&build_report(&fake())).unwrap();
        let second = serde_json::to_vec_pretty(&build_report(&fake())).unwrap();
        assert_eq!(first, second);
        let report = build_report(&fake());
        assert_eq!(
            report.mode_summary["scheduler"],
            if cfg!(feature = "ebpf") {
                ModeAvailability::PermissionRequired
            } else {
                ModeAvailability::NotIncludedInBuild
            }
        );
    }

    #[test]
    fn missing_prerequisites_and_unsupported_platform_are_separate() {
        let mut missing = fake();
        missing.btf = false;
        let report = build_report(&missing);
        assert_eq!(
            report.mode_summary["tcp"],
            if cfg!(feature = "ebpf") {
                ModeAvailability::MissingPrerequisite
            } else {
                ModeAvailability::NotIncludedInBuild
            }
        );

        let mut other = fake();
        other.os = "macos".to_owned();
        let report = build_report(&other);
        assert_eq!(
            report.mode_summary["proc"],
            ModeAvailability::UnsupportedPlatform
        );
        assert_eq!(
            report.mode_summary["scheduler"],
            ModeAvailability::UnsupportedPlatform
        );
    }

    #[test]
    fn proc_partial_visibility_and_tracepoint_loss_are_reported() {
        let mut snapshot = fake();
        snapshot.proc_partial_visibility = ProbeResult::Restricted;
        snapshot
            .scheduler_tracepoints
            .insert("sched_switch".to_owned(), ProbeResult::Missing);
        let report = build_report(&snapshot);
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "proc.visibility")
                .unwrap()
                .status,
            CheckStatus::Warn
        );
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "scheduler.tracepoint.sched_switch")
                .unwrap()
                .status,
            CheckStatus::Warn
        );
    }

    #[test]
    fn passive_report_marks_live_attachment_not_tested() {
        let report = build_report(&fake());
        for id in ["scheduler.live_attachment", "tcp.live_attachment"] {
            assert_eq!(
                report
                    .checks
                    .iter()
                    .find(|check| check.id == id)
                    .unwrap()
                    .status,
                CheckStatus::NotTested
            );
        }
    }

    #[test]
    fn ring_buffer_and_lockdown_parsers_are_bounded() {
        assert_eq!(kernel_likely_has_ring_buffer("5.8.0"), Some(true));
        assert_eq!(kernel_likely_has_ring_buffer("5.7.19"), Some(false));
        assert_eq!(kernel_likely_has_ring_buffer("not-a-kernel"), None);
        assert_eq!(
            selected_lockdown("none [integrity] confidentiality"),
            "integrity"
        );
    }

    #[test]
    fn requested_check_exit_policy_is_stable() {
        assert!(!matches!(
            ebpf_mode(true, true, true, true, true, true, false, Some(false)),
            ModeAvailability::MissingPrerequisite
        ));
        assert_eq!(
            ebpf_mode(true, true, true, true, true, true, false, Some(false)),
            ModeAvailability::PermissionRequired
        );
        assert_eq!(
            ebpf_mode(true, false, true, true, true, true, false, Some(true)),
            ModeAvailability::MissingPrerequisite
        );
    }
}
