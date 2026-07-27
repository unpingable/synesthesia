//! Shared, passive eBPF prerequisite definitions.
//!
//! Live collectors and `synesthesia doctor` consume these same constants so
//! diagnostic names cannot drift away from the programs that actually attach.

pub const SUPPORTED_ARCHITECTURE: &str = "x86_64";
pub const SCHEDULER_COLLECTOR: &str = "synesthesia-scheduler-collector";
pub const TCP_COLLECTOR: &str = "synesthesia-tcp-collector";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TracepointSpec {
    pub program: &'static str,
    pub group: &'static str,
    pub name: &'static str,
}

pub const SCHEDULER_TRACEPOINTS: [TracepointSpec; 4] = [
    TracepointSpec {
        program: "synesthesia_sched_switch",
        group: "sched",
        name: "sched_switch",
    },
    TracepointSpec {
        program: "synesthesia_sched_wakeup",
        group: "sched",
        name: "sched_wakeup",
    },
    TracepointSpec {
        program: "synesthesia_sched_wakeup_new",
        group: "sched",
        name: "sched_wakeup_new",
    },
    TracepointSpec {
        program: "synesthesia_sched_migrate",
        group: "sched",
        name: "sched_migrate_task",
    },
];

pub const TCP_TRACEPOINTS: [TracepointSpec; 3] = [
    TracepointSpec {
        program: "synesthesia_tcp_retransmit",
        group: "tcp",
        name: "tcp_retransmit_skb",
    },
    TracepointSpec {
        program: "synesthesia_tcp_reset_sent",
        group: "tcp",
        name: "tcp_send_reset",
    },
    TracepointSpec {
        program: "synesthesia_tcp_reset_received",
        group: "tcp",
        name: "tcp_receive_reset",
    },
];

pub const SCHEDULER_BTF_TYPES: [&str; 3] = [
    "trace_event_raw_sched_switch",
    "trace_event_raw_sched_wakeup_template",
    "trace_event_raw_sched_migrate_task",
];

pub const TCP_BTF_TYPES: [&str; 2] = [
    "trace_event_raw_tcp_event_sk_skb",
    "trace_event_raw_tcp_event_sk",
];
