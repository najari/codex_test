use anyhow::{ensure, Result};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cancelled")
    }
}
impl std::error::Error for Cancelled {}

#[derive(Clone, Default)]
pub struct Cancellation(pub Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Relaxed) {
            Err(Cancelled.into())
        } else {
            Ok(())
        }
    }
}
pub trait Clock {
    fn now(&self) -> Duration;
    fn sleep(&mut self, duration: Duration);
}
pub struct RealClock {
    start: Instant,
}
impl Default for RealClock {
    fn default() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}
impl Clock for RealClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[derive(Debug)]
pub enum Control {
    Pause,
    Resume,
    Stop,
}
pub struct Scheduler<C: Clock> {
    clock: C,
    anchor: Option<(i64, Duration)>,
    previous: Option<i64>,
    speed: f64,
    no_wait: bool,
    immediate_regression: bool,
    paused: Option<Duration>,
    paused_total: Duration,
}
impl<C: Clock> Scheduler<C> {
    pub fn new(clock: C, speed: f64, no_wait: bool, immediate_regression: bool) -> Result<Self> {
        ensure!(
            speed.is_finite() && speed > 0.0,
            "speed must be finite and positive"
        );
        Ok(Self {
            clock,
            anchor: None,
            previous: None,
            speed,
            no_wait,
            immediate_regression,
            paused: None,
            paused_total: Duration::ZERO,
        })
    }
    pub fn control(&mut self, control: Control, cancel: &Cancellation) {
        match control {
            Control::Pause => {
                if self.paused.is_none() {
                    self.paused = Some(self.clock.now());
                }
            }
            Control::Resume => {
                if let Some(at) = self.paused.take() {
                    self.paused_total += self.clock.now() - at;
                }
            }
            Control::Stop => cancel.cancel(),
        }
    }
    pub fn wait(
        &mut self,
        timestamp: i64,
        cancel: &Cancellation,
        controls: Option<&Receiver<Control>>,
    ) -> Result<()> {
        let regression = self.previous.is_some_and(|p| timestamp < p);
        ensure!(
            !regression || self.immediate_regression,
            "replay timestamp regression: {:?} -> {timestamp}; use --on-regression immediate",
            self.previous
        );
        self.previous = Some(timestamp);
        let (origin, wall) = *self.anchor.get_or_insert((timestamp, self.clock.now()));
        let scaled = (timestamp.saturating_sub(origin).max(0) as f64 / 1e9) / self.speed;
        let offset = Duration::try_from_secs_f64(scaled)
            .map_err(|_| anyhow::anyhow!("scaled replay duration is outside supported range"))?;
        loop {
            cancel.check()?;
            if let Some(controls) = controls {
                while let Ok(control) = controls.try_recv() {
                    self.control(control, cancel);
                }
            }
            cancel.check()?;
            if self.paused.is_some() {
                self.clock.sleep(Duration::from_millis(10));
                continue;
            }
            if self.no_wait || (regression && self.immediate_regression) {
                return Ok(());
            }
            let deadline = wall
                .checked_add(offset)
                .and_then(|d| d.checked_add(self.paused_total))
                .ok_or_else(|| anyhow::anyhow!("replay deadline overflow"))?;
            let now = self.clock.now();
            if now >= deadline {
                return Ok(());
            }
            self.clock
                .sleep((deadline - now).min(Duration::from_millis(10)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct FakeClock {
        now: Duration,
    }
    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            self.now
        }
        fn sleep(&mut self, d: Duration) {
            self.now += d;
        }
    }
    #[test]
    fn deadlines_speed_and_pause() {
        let cancel = Cancellation::default();
        let mut s = Scheduler::new(FakeClock::default(), 2.0, false, false).unwrap();
        s.wait(100, &cancel, None).unwrap();
        s.wait(1_000_000_100, &cancel, None).unwrap();
        assert_eq!(s.clock.now, Duration::from_millis(500));
        s.control(Control::Pause, &cancel);
        s.clock.sleep(Duration::from_secs(3));
        s.control(Control::Resume, &cancel);
        s.wait(2_000_000_100, &cancel, None).unwrap();
        assert_eq!(s.clock.now, Duration::from_secs(4));
        s.wait(2_000_000_100, &cancel, None).unwrap();
        assert_eq!(s.clock.now, Duration::from_secs(4));
    }
    #[test]
    fn regression_and_cancel() {
        let cancel = Cancellation::default();
        let mut s = Scheduler::new(FakeClock::default(), 1.0, true, false).unwrap();
        s.wait(10, &cancel, None).unwrap();
        assert!(s.wait(9, &cancel, None).is_err());
        cancel.cancel();
        assert!(s.wait(11, &cancel, None).unwrap_err().is::<Cancelled>());
        assert!(Scheduler::new(FakeClock::default(), f64::NAN, false, false).is_err());
    }
}
