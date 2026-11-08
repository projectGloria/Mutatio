use crate::config::StateAppearance;
use std::time::{Duration, Instant};

pub struct StateTransition {
    pub start_appearance: StateAppearance,
    pub target_appearance: StateAppearance,
    pub start_time: Instant,
    pub duration: Duration,
    pub is_active: bool,
}

impl StateTransition {
    pub fn new(current: StateAppearance, target: StateAppearance, duration_ms: u64) -> Self {
        Self {
            start_appearance: current,
            target_appearance: target,
            start_time: Instant::now(),
            duration: Duration::from_millis(duration_ms),
            is_active: true,
        }
    }

    /// Evaluates current interpolated appearance at the current time
    pub fn step(&mut self) -> (StateAppearance, bool) {
        if !self.is_active {
            return (self.target_appearance.clone(), false);
        }

        let elapsed = self.start_time.elapsed();
        if elapsed >= self.duration {
            self.is_active = false;
            return (self.target_appearance.clone(), false);
        }

        let progress = (elapsed.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0);
        // Ease-out cubic formula: 1 - (1 - t)^3
        let eased = 1.0 - (1.0 - progress).powi(3);

        let interpolated_color = self.start_appearance.color.lerp(self.target_appearance.color, eased);

        // Transition state type when past halfway point
        let state = if progress < 0.5 {
            self.start_appearance.state
        } else {
            self.target_appearance.state
        };

        let current = StateAppearance {
            state,
            color: interpolated_color,
            show_line: self.target_appearance.show_line,
        };

        (current, true)
    }
}
