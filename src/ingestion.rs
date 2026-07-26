use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};

use crate::event::NormalizedEvent;

#[derive(Clone)]
pub struct Ingress {
    sender: Sender<NormalizedEvent>,
    dropped: Arc<AtomicU64>,
}

pub struct EventBuffer {
    receiver: Receiver<NormalizedEvent>,
    dropped: Arc<AtomicU64>,
}

pub fn event_buffer(capacity: usize) -> (Ingress, EventBuffer) {
    assert!(capacity > 0, "event buffer capacity must be non-zero");
    let (sender, receiver) = bounded(capacity);
    let dropped = Arc::new(AtomicU64::new(0));
    (
        Ingress {
            sender,
            dropped: Arc::clone(&dropped),
        },
        EventBuffer { receiver, dropped },
    )
}

impl Ingress {
    pub fn submit(&self, event: NormalizedEvent) -> bool {
        match self.sender.try_send(event) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }
}

impl EventBuffer {
    pub fn drain_into(&self, destination: &mut Vec<NormalizedEvent>, limit: usize) {
        destination.extend(self.receiver.try_iter().take(limit));
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use crate::source::demo::DemoSource;

    use super::*;

    #[test]
    fn bounded_buffer_drops_without_growing() {
        let (ingress, buffer) = event_buffer(3);
        for event in DemoSource::new(1).take(10) {
            ingress.submit(event);
        }
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.dropped(), 7);
        let mut drained = Vec::new();
        buffer.drain_into(&mut drained, 99);
        assert_eq!(drained.len(), 3);
        assert!(buffer.is_empty());
    }
}
