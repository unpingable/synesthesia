/*
 * Narrow TCP pathology sensor for Synesthesia.
 *
 * The tracepoint classes already expose endpoint metadata. This program only
 * copies those fixed fields into a bounded ring buffer; it never reads packet
 * payloads, socket contents, command lines, or stack traces.
 */

typedef unsigned char u8;
typedef unsigned short u16;
typedef unsigned int u32;
typedef unsigned long long u64;

#define SEC(name) __attribute__((section(name), used))
#define BPF_MAP_TYPE_PERCPU_ARRAY 6
#define BPF_MAP_TYPE_RINGBUF 27
#define AF_INET 2
#define AF_INET6 10
#define TCP_EVENT_VERSION 1

enum tcp_pathology_kind {
    EVENT_RETRANSMIT = 1,
    EVENT_RESET_SENT = 2,
    EVENT_RESET_RECEIVED = 3,
};

struct bpf_map_def {
    u32 type;
    u32 key_size;
    u32 value_size;
    u32 max_entries;
    u32 map_flags;
};

struct bpf_map_def SEC("maps") TCP_EVENTS = {
    .type = BPF_MAP_TYPE_RINGBUF,
    .max_entries = 1 << 20,
};

struct bpf_map_def SEC("maps") TCP_LOSSES = {
    .type = BPF_MAP_TYPE_PERCPU_ARRAY,
    .key_size = sizeof(u32),
    .value_size = sizeof(u64),
    .max_entries = 1,
};

static void *(*bpf_map_lookup_elem)(void *map, const void *key) = (void *)1;
static u64 (*bpf_ktime_get_ns)(void) = (void *)5;
static u32 (*bpf_get_smp_processor_id)(void) = (void *)8;
static long (*bpf_probe_read_kernel)(void *dst, u32 size, const void *unsafe_ptr) =
    (void *)113;
static void *(*bpf_ringbuf_reserve)(void *ringbuf, u64 size, u64 flags) =
    (void *)131;
static void (*bpf_ringbuf_submit)(void *data, u64 flags) = (void *)132;
static void (*bpf_ringbuf_discard)(void *data, u64 flags) = (void *)133;

#define BPF_CORE_READ_INTO(dst, src, field)                                  \
    bpf_probe_read_kernel(&(dst), sizeof(dst),                               \
                          __builtin_preserve_access_index(&((src)->field)))

#define BPF_CORE_READ_BYTES(dst, size, src, field)                            \
    bpf_probe_read_kernel((dst), (size),                                     \
                          __builtin_preserve_access_index(&((src)->field)))

struct trace_entry {
    u16 type;
    u8 flags;
    u8 preempt_count;
    int pid;
} __attribute__((preserve_access_index));

struct trace_event_raw_tcp_event_sk_skb {
    struct trace_entry ent;
    const void *skbaddr;
    const void *skaddr;
    int state;
    u16 sport;
    u16 dport;
    u16 family;
    u8 saddr[4];
    u8 daddr[4];
    u8 saddr_v6[16];
    u8 daddr_v6[16];
} __attribute__((preserve_access_index));

struct trace_event_raw_tcp_event_sk {
    struct trace_entry ent;
    const void *skaddr;
    u16 sport;
    u16 dport;
    u16 family;
    u8 saddr[4];
    u8 daddr[4];
    u8 saddr_v6[16];
    u8 daddr_v6[16];
    u64 sock_cookie;
} __attribute__((preserve_access_index));

struct tcp_pathology_event {
    u64 timestamp_ns;
    u16 version;
    u8 kind;
    u8 family;
    u32 cpu;
    u16 source_port;
    u16 destination_port;
    u32 socket_state;
    u8 source_address[16];
    u8 destination_address[16];
};

static __attribute__((always_inline)) void account_loss(void)
{
    u32 key = 0;
    u64 *losses = bpf_map_lookup_elem(&TCP_LOSSES, &key);
    if (losses)
        *losses += 1;
}

static __attribute__((always_inline)) struct tcp_pathology_event *
reserve_event(u8 kind)
{
    struct tcp_pathology_event *event =
        bpf_ringbuf_reserve(&TCP_EVENTS, sizeof(*event), 0);
    if (!event) {
        account_loss();
        return 0;
    }
    __builtin_memset(event, 0, sizeof(*event));
    event->timestamp_ns = bpf_ktime_get_ns();
    event->version = TCP_EVENT_VERSION;
    event->kind = kind;
    event->cpu = bpf_get_smp_processor_id();
    return event;
}

static __attribute__((always_inline)) int read_addresses_sk_skb(
    struct tcp_pathology_event *event,
    struct trace_event_raw_tcp_event_sk_skb *ctx)
{
    if (event->family == AF_INET)
        return BPF_CORE_READ_BYTES(event->source_address, 4, ctx, saddr) ||
               BPF_CORE_READ_BYTES(event->destination_address, 4, ctx, daddr);
    if (event->family == AF_INET6)
        return BPF_CORE_READ_BYTES(event->source_address, 16, ctx, saddr_v6) ||
               BPF_CORE_READ_BYTES(event->destination_address, 16, ctx, daddr_v6);
    return 1;
}

static __attribute__((always_inline)) int emit_sk_skb(
    struct trace_event_raw_tcp_event_sk_skb *ctx, u8 kind)
{
    u16 family;
    u16 sport;
    u16 dport;
    int state;
    struct tcp_pathology_event *event = reserve_event(kind);
    if (!event)
        return 0;

    if (BPF_CORE_READ_INTO(family, ctx, family) ||
        BPF_CORE_READ_INTO(sport, ctx, sport) ||
        BPF_CORE_READ_INTO(dport, ctx, dport) ||
        BPF_CORE_READ_INTO(state, ctx, state)) {
        account_loss();
        bpf_ringbuf_discard(event, 0);
        return 0;
    }
    event->family = family;
    event->source_port = sport;
    event->destination_port = dport;
    event->socket_state = state;
    if (read_addresses_sk_skb(event, ctx)) {
        account_loss();
        bpf_ringbuf_discard(event, 0);
        return 0;
    }
    bpf_ringbuf_submit(event, 0);
    return 0;
}

SEC("tracepoint/tcp/tcp_retransmit_skb")
int synesthesia_tcp_retransmit(struct trace_event_raw_tcp_event_sk_skb *ctx)
{
    return emit_sk_skb(ctx, EVENT_RETRANSMIT);
}

SEC("tracepoint/tcp/tcp_send_reset")
int synesthesia_tcp_reset_sent(struct trace_event_raw_tcp_event_sk_skb *ctx)
{
    return emit_sk_skb(ctx, EVENT_RESET_SENT);
}

SEC("tracepoint/tcp/tcp_receive_reset")
int synesthesia_tcp_reset_received(struct trace_event_raw_tcp_event_sk *ctx)
{
    u16 family;
    u16 sport;
    u16 dport;
    struct tcp_pathology_event *event = reserve_event(EVENT_RESET_RECEIVED);
    if (!event)
        return 0;

    if (BPF_CORE_READ_INTO(family, ctx, family) ||
        BPF_CORE_READ_INTO(sport, ctx, sport) ||
        BPF_CORE_READ_INTO(dport, ctx, dport)) {
        account_loss();
        bpf_ringbuf_discard(event, 0);
        return 0;
    }
    event->family = family;
    event->source_port = sport;
    event->destination_port = dport;
    if (family == AF_INET) {
        if (BPF_CORE_READ_BYTES(event->source_address, 4, ctx, saddr) ||
            BPF_CORE_READ_BYTES(event->destination_address, 4, ctx, daddr)) {
            account_loss();
            bpf_ringbuf_discard(event, 0);
            return 0;
        }
    } else if (family == AF_INET6) {
        if (BPF_CORE_READ_BYTES(event->source_address, 16, ctx, saddr_v6) ||
            BPF_CORE_READ_BYTES(event->destination_address, 16, ctx, daddr_v6)) {
            account_loss();
            bpf_ringbuf_discard(event, 0);
            return 0;
        }
    } else {
        account_loss();
        bpf_ringbuf_discard(event, 0);
        return 0;
    }
    bpf_ringbuf_submit(event, 0);
    return 0;
}

char LICENSE[] SEC("license") = "GPL";
