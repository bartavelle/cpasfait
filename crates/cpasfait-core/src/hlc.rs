//! Hybrid logical clock. Gives a total order on operations across devices that
//! stays close to wall-clock time and never goes backwards on one device.

use crate::ids::DeviceId;
use serde::{Deserialize, Serialize};

/// A timestamp. Ordering is (wall, counter, device): unique per device, total overall.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Hlc {
    /// Milliseconds since the Unix epoch.
    pub wall: i64,
    pub counter: u32,
    pub device: DeviceId,
}

/// The mutable clock state of one device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockState {
    pub wall: i64,
    pub counter: u32,
}

impl ClockState {
    pub const ZERO: ClockState = ClockState { wall: 0, counter: 0 };

    /// Produce a new timestamp for a local event.
    pub fn tick(&mut self, now_ms: i64, device: DeviceId) -> Hlc {
        if now_ms > self.wall {
            self.wall = now_ms;
            self.counter = 0;
        } else {
            self.counter += 1;
        }
        Hlc {
            wall: self.wall,
            counter: self.counter,
            device,
        }
    }

    /// Account for a timestamp received from another device, so that later local
    /// events are ordered after it.
    pub fn observe(&mut self, remote: &Hlc) {
        if (remote.wall, remote.counter) > (self.wall, self.counter) {
            self.wall = remote.wall;
            self.counter = remote.counter;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_even_if_wall_clock_goes_back() {
        let d = DeviceId::new();
        let mut c = ClockState::ZERO;
        let a = c.tick(1000, d);
        let b = c.tick(900, d);
        let e = c.tick(1000, d);
        assert!(a < b && b < e);
        let f = c.tick(2000, d);
        assert_eq!((f.wall, f.counter), (2000, 0));
    }

    #[test]
    fn observe_orders_after_remote() {
        let d1 = DeviceId::new();
        let d2 = DeviceId::new();
        let mut c1 = ClockState::ZERO;
        let mut c2 = ClockState::ZERO;
        let remote = c1.tick(5000, d1);
        c2.observe(&remote);
        let local = c2.tick(100, d2);
        assert!(local > remote);
    }
}
