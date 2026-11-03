use std::time::Instant;

/// The one monotonic clock for a recording. Clicks, keys, frames and audio
/// are all stamped in milliseconds from `start`.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    start: Instant,
}

impl Clock {
    pub fn start() -> Self {
        Self { start: Instant::now() }
    }

    pub fn now_ms(&self) -> u64 {
        self.ms_at(Instant::now())
    }

    pub fn ms_at(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.start).as_millis() as u64
    }
}
