use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CBState {
    Closed,
    Open,
}

pub struct CircuitBreaker {
    pub state: CBState,
    max_failures: usize,
    opened_at: Option<Instant>,
    window: Duration,
    failures: usize,
}

impl CircuitBreaker {
    pub fn new(max_failures: usize, window: Duration) -> Self {
        CircuitBreaker {
            state: CBState::Closed,
            max_failures,
            opened_at: None,
            window,
            failures: 0,
        }
    }

    pub fn on_failure(&mut self) {
        self.failures += 1;
        if self.failures >= self.max_failures {
            self.state = CBState::Open;
            self.opened_at = Some(Instant::now());
        }
    }

    pub fn is_allowed(&self) -> bool {
        if self.state == CBState::Open {
            if let Some(s) = self.opened_at {
                if s.elapsed() > self.window {
                    return true; // allow after cooldown
                }
            }
            false
        } else {
            true
        }
    }
}
