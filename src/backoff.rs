use std::time::Duration;

pub struct Backoff {
    initial: Duration,
    current: Duration,
    max: Duration,
    warm_after: Duration,
}

impl Backoff {
    pub fn new(initial: Duration, max: Duration, warm_after: Duration) -> Self {
        Self {
            initial,
            current: initial,
            max,
            warm_after,
        }
    }

    pub fn advance(&mut self, attempt_duration: Duration) -> Duration {
        self.current = if attempt_duration > self.warm_after {
            self.initial
        } else {
            std::cmp::min(self.current * 2, self.max)
        };
        self.current
    }
}
