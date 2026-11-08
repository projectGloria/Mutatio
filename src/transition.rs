use crate::config::StateAppearance;
use std::time::{Duration, Instant};

pub struct StateTransition {
    from: StateAppearance,
    to: StateAppearance,
    start_time: Instant,
    duration: Duration,
}

impl StateTransition {
    pub fn new(from: StateAppearance, to: StateAppearance, duration_ms: u64) -> Self {
        Self {
            from,
            to,
            start_time: Instant::now(),
            duration: Duration::from_millis(duration_ms),
        }
    }

    pub fn target(&self) -> StateAppearance {
        self.to
    }

    /// Returns the interpolated appearance for "now" and whether the animation is still running.
    pub fn step(&self) -> (StateAppearance, bool) {
        let elapsed = self.start_time.elapsed();
        // `>=` also covers a zero duration, which would otherwise divide by zero below.
        if elapsed >= self.duration {
            return (self.to, false);
        }

        let progress = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        // Ease-out cubic: 1 - (1 - t)^3
        let eased = 1.0 - (1.0 - progress).powi(3);

        // The brush type (acrylic / solid / blur) can't be interpolated, so it flips at the midpoint.
        let state = if progress < 0.5 { self.from.state } else { self.to.state };

        let current = StateAppearance {
            state,
            color: self.from.color.lerp(self.to.color, eased),
            show_line: self.to.show_line,
        };
        (current, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TaskbarState;

    #[test]
    fn zero_duration_finishes_immediately() {
        let a = StateAppearance::preset(TaskbarState::Clear);
        let b = StateAppearance::preset(TaskbarState::Opaque);
        let (now, active) = StateTransition::new(a, b, 0).step();
        assert!(!active);
        assert_eq!(now, b);
    }

    #[test]
    fn starts_at_source_state() {
        let a = StateAppearance::preset(TaskbarState::Clear);
        let b = StateAppearance::preset(TaskbarState::Opaque);
        let (now, active) = StateTransition::new(a, b, 60_000).step();
        assert!(active);
        assert_eq!(now.state, TaskbarState::Clear);
    }
}
