/*
 * Minimal scheduler tracepoint sensor for Synesthesia.
 *
 * This file deliberately avoids libbpf headers: Clang emits CO-RE relocation
 * metadata for the preserve_access_index tracepoint types below, and Aya owns
 * loading, relocation, attachment, and ring-buffer consumption.
 */

typedef unsigned int u32;
typedef unsigned long long u64;
typedef long long s64;

#define SEC(name) __attribute__((section(name), used))
#define BPF_MAP_TYPE_PERCPU_ARRAY 6
#define BPF_MAP_TYPE_RINGBUF 27
#define UNKNOWN_CPU ((u32)-1)

enum scheduler_event_kind {
    EVENT_SWITCH = 1,
    EVENT_WAKEUP = 2,
    EVENT_WAKEUP_NEW = 3,
    EVENT_MIGRATE = 4,
};

struct bpf_map_def {
    u32 type;
    u32 key_size;
    u32 value_size;
    u32 max_entries;
    u32 map_flags;
};

struct bpf_map_def SEC("maps") EVENTS = {
    .type = BPF_MAP_TYPE_RINGBUF,
    .max_entries = 1 << 20,
};

struct bpf_map_def SEC("maps") LOSSES = {
    .type = BPF_MAP_TYPE_PERCPU_ARRAY,
    .key_size = sizeof(u32),
    .value_size = sizeof(u64),
    .max_entries = 1,
};

static void *(*bpf_map_lookup_elem)(void *map, const void *key) = (void *)1;
static u64 (*bpf_ktime_get_ns)(void) = (void *)5;
static u32 (*bpf_get_smp_processor_id)(void) = (void *)8;
static u64 (*bpf_get_current_pid_tgid)(void) = (void *)14;
static long (*bpf_probe_read_kernel)(void *dst, u32 size, const void *unsafe_ptr) =
    (void *)113;
static void *(*bpf_ringbuf_reserve)(void *ringbuf, u64 size, u64 flags) =
    (void *)131;
static void (*bpf_ringbuf_submit)(void *data, u64 flags) = (void *)132;
static void (*bpf_ringbuf_discard)(void *data, u64 flags) = (void *)133;

#define BPF_CORE_READ_INTO(dst, src, field)                                  \
    bpf_probe_read_kernel(&(dst), sizeof(dst),                               \
                          __builtin_preserve_access_index(&((src)->field)))

struct trace_entry {
    unsigned short type;
    unsigned char flags;
    unsigned char preempt_count;
    int pid;
} __attribute__((preserve_access_index));

struct trace_event_raw_sched_switch {
    struct trace_entry ent;
    char prev_comm[16];
    int prev_pid;
    int prev_prio;
    long prev_state;
    char next_comm[16];
    int next_pid;
    int next_prio;
} __attribute__((preserve_access_index));

struct trace_event_raw_sched_wakeup_template {
    struct trace_entry ent;
    char comm[16];
    int pid;
    int prio;
    int target_cpu;
} __attribute__((preserve_access_index));

struct trace_event_raw_sched_migrate_task {
    struct trace_entry ent;
    char comm[16];
    int pid;
    int prio;
    int orig_cpu;
    int dest_cpu;
} __attribute__((preserve_access_index));

struct scheduler_event {
    u64 timestamp_ns;
    u32 kind;
    u32 cpu;
    u32 source_cpu;
    u32 target_cpu;
    u32 pid;
    u32 previous_pid;
    u32 next_pid;
    s64 previous_state;
};

static __attribute__((always_inline)) void account_loss(void)
{
    u32 key = 0;
    u64 *losses = bpf_map_lookup_elem(&LOSSES, &key);
    if (losses)
        *losses += 1;
}

static __attribute__((always_inline)) struct scheduler_event *
reserve_event(u32 kind)
{
    struct scheduler_event *event =
        bpf_ringbuf_reserve(&EVENTS, sizeof(*event), 0);
    if (!event) {
        account_loss();
        return 0;
    }

    event->timestamp_ns = bpf_ktime_get_ns();
    event->kind = kind;
    event->cpu = bpf_get_smp_processor_id();
    event->source_cpu = UNKNOWN_CPU;
    event->target_cpu = UNKNOWN_CPU;
    event->pid = 0;
    event->previous_pid = 0;
    event->next_pid = 0;
    event->previous_state = 0;
    return event;
}

SEC("tracepoint/sched/sched_switch")
int synesthesia_sched_switch(struct trace_event_raw_sched_switch *ctx)
{
    int previous_pid;
    int next_pid;
    long previous_state;
    struct scheduler_event *event = reserve_event(EVENT_SWITCH);
    if (!event)
        return 0;

    if (BPF_CORE_READ_INTO(previous_pid, ctx, prev_pid) ||
        BPF_CORE_READ_INTO(next_pid, ctx, next_pid) ||
        BPF_CORE_READ_INTO(previous_state, ctx, prev_state)) {
        account_loss();
        bpf_ringbuf_discard(event, 0);
        return 0;
    }
    event->source_cpu = event->cpu;
    event->target_cpu = event->cpu;
    event->previous_pid = previous_pid;
    event->next_pid = next_pid;
    event->pid = event->next_pid;
    event->previous_state = previous_state;
    bpf_ringbuf_submit(event, 0);
    return 0;
}

static __attribute__((always_inline)) int
emit_wakeup(struct trace_event_raw_sched_wakeup_template *ctx, u32 kind)
{
    int target_cpu;
    int pid;
    struct scheduler_event *event = reserve_event(kind);
    if (!event)
        return 0;

    if (BPF_CORE_READ_INTO(target_cpu, ctx, target_cpu) ||
        BPF_CORE_READ_INTO(pid, ctx, pid)) {
        account_loss();
        bpf_ringbuf_discard(event, 0);
        return 0;
    }
    event->source_cpu = event->cpu;
    event->target_cpu = target_cpu;
    event->pid = pid;
    event->previous_pid = (u32)bpf_get_current_pid_tgid();
    event->next_pid = event->pid;
    bpf_ringbuf_submit(event, 0);
    return 0;
}

SEC("tracepoint/sched/sched_wakeup")
int synesthesia_sched_wakeup(struct trace_event_raw_sched_wakeup_template *ctx)
{
    return emit_wakeup(ctx, EVENT_WAKEUP);
}

SEC("tracepoint/sched/sched_wakeup_new")
int synesthesia_sched_wakeup_new(struct trace_event_raw_sched_wakeup_template *ctx)
{
    return emit_wakeup(ctx, EVENT_WAKEUP_NEW);
}

SEC("tracepoint/sched/sched_migrate_task")
int synesthesia_sched_migrate(struct trace_event_raw_sched_migrate_task *ctx)
{
    int source_cpu;
    int target_cpu;
    int pid;
    struct scheduler_event *event = reserve_event(EVENT_MIGRATE);
    if (!event)
        return 0;

    if (BPF_CORE_READ_INTO(source_cpu, ctx, orig_cpu) ||
        BPF_CORE_READ_INTO(target_cpu, ctx, dest_cpu) ||
        BPF_CORE_READ_INTO(pid, ctx, pid)) {
        account_loss();
        bpf_ringbuf_discard(event, 0);
        return 0;
    }
    event->source_cpu = source_cpu;
    event->target_cpu = target_cpu;
    event->pid = pid;
    bpf_ringbuf_submit(event, 0);
    return 0;
}

char LICENSE[] SEC("license") = "GPL";
