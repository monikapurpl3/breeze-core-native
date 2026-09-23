//! Keeping unit connections open between uses.
//!
//! A unit closes a connection exactly 30 s after the last request it received,
//! and the next request then pays for a new one: connect, handshake, and the
//! one-second pause the protocol demands before the unit will listen. Opening
//! the app a minute after closing it paid that on every unit, which is most of
//! why a fresh open felt slower than a second one.
//!
//! So for a while after the server was last used (`BREEZE_KEEP_WARM`, half an
//! hour by default) each unit that has gone quiet is read once, which resets its
//! timer. That is one small request per unit every 20-25 s, only in that window,
//! and never to a unit that is busy -- whoever is using it is keeping it warm --
//! or one that has just failed to answer.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::state::{AppState, KeepWarm};

/// Read a unit once it has been quiet this long. Its idle timer fires at 30 s;
/// this leaves room for the tick below and for the read itself.
pub const WARM_AFTER: Duration = Duration::from_secs(20);

/// How often to look. Which units are actually read is decided per unit, by how
/// long each has been quiet, so a busy unit is never read for nothing.
const TICK: Duration = Duration::from_secs(5);

/// Start keeping units warm, unless the setting says never. Returns the thread,
/// or `None` when there is nothing to run.
pub fn spawn_keep_warm(state: Arc<AppState>) -> Option<JoinHandle<()>> {
    if state.settings.keep_warm == KeepWarm::Off {
        return None;
    }
    Some(std::thread::spawn(move || loop {
        std::thread::sleep(TICK);
        if state.settings.keep_warm.active(state.activity.since_last()) {
            state.manager.keep_warm(WARM_AFTER);
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_is_read_well_inside_its_idle_timer() {
        // The unit closes at 30 s. The latest a read can land is WARM_AFTER plus
        // one TICK, and it must still leave a margin for the round-trip.
        let latest = WARM_AFTER + TICK;
        assert!(
            latest + Duration::from_secs(2) < Duration::from_secs(30),
            "a quiet unit could be read at {latest:?}, too close to its 30 s timer"
        );
    }
}
