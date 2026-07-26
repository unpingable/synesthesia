#![forbid(unsafe_code)]

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn main() {
    eprintln!("synesthesia-scheduler-collector supports only Linux x86_64");
    std::process::exit(2);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn main() -> anyhow::Result<()> {
    use std::{
        io::{self, BufWriter, Write},
        thread,
        time::{Duration, Instant},
    };

    use synesthesia::source::{scheduler_ipc::SchedulerAggregator, scheduler_live::LiveScheduler};

    const WINDOW: Duration = Duration::from_millis(33);
    const MAX_RAW_PER_POLL: usize = 4_096;

    let mut capture = LiveScheduler::attach()?;
    let stdout = io::stdout();
    let mut output = BufWriter::with_capacity(128 * 1024, stdout.lock());
    let mut aggregator = SchedulerAggregator::new();
    let mut window_started = Instant::now();
    let mut timestamp_ns = 0;

    loop {
        let mut received = false;
        for _ in 0..MAX_RAW_PER_POLL {
            let Some(item) = capture.next_event() else {
                break;
            };
            let event = item?;
            timestamp_ns = timestamp_ns.max(event.timestamp_ns);
            aggregator.ingest(event);
            received = true;
        }

        if window_started.elapsed() >= WINDOW {
            let kernel_drops = capture.kernel_ring_drops()?;
            for pulse in aggregator.flush(timestamp_ns, kernel_drops) {
                pulse.write_to(&mut output)?;
            }
            output.flush()?;
            window_started = Instant::now();
        } else if !received {
            thread::sleep(Duration::from_millis(2));
        }
    }
}
