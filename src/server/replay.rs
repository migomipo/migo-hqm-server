//! Replay tick retention and playback queue management.

use std::collections::VecDeque;
use std::rc::Rc;

use tracing::warn;

use crate::game::{PlayerId, Team};
use crate::history::History;
use crate::protocol::ObjectPacket;

/// The player a skater object belonged to when a tick was recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkaterOwner {
    pub(super) player_id: PlayerId,
    pub(super) player_name: Rc<str>,
    pub(super) team: Team,
}

/// The owners of the skater objects in a tick, by object slot.
pub(crate) type SkaterOwners = [Option<SkaterOwner>; 32];

#[derive(Clone, Debug)]
pub(super) struct ReplayTick {
    pub(super) game_step: u32,
    /// Shared with the packet history, which the tick is sent from.
    pub(super) packets: Rc<[ObjectPacket; 32]>,
    /// Shared with the previous tick while the owners stay the same.
    pub(super) owners: Rc<SkaterOwners>,
}

pub struct HQMTickHistory {
    current_step: u32,
    pending_replay: VecDeque<(Option<PlayerId>, ReplayTick)>,
    retained_ticks: History<ReplayTick>,
}

impl HQMTickHistory {
    pub(crate) fn new() -> Self {
        Self {
            current_step: u32::MAX,
            pending_replay: Default::default(),
            retained_ticks: History::new(0),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.pending_replay.clear();
        self.retained_ticks.clear();
        self.current_step = u32::MAX;
    }

    pub fn is_in_replay(&self) -> bool {
        !self.pending_replay.is_empty()
    }

    pub(crate) fn game_step(&self) -> u32 {
        self.current_step
    }

    pub(crate) fn set_history_capacity(&mut self, history_capacity: usize) {
        self.retained_ticks.set_capacity(history_capacity);
    }

    pub(crate) fn advance_step(&mut self) {
        self.current_step = self.current_step.wrapping_add(1);
    }

    pub(crate) fn record_tick(
        &mut self,
        packets: impl Into<Rc<[ObjectPacket; 32]>>,
        owners: SkaterOwners,
    ) {
        let owners = match self.retained_ticks.newest() {
            Some(previous) if *previous.owners == owners => previous.owners.clone(),
            _ => Rc::new(owners),
        };
        self.retained_ticks.push(ReplayTick {
            game_step: self.current_step,
            packets: packets.into(),
            owners,
        });
    }

    /// Queues the retained ticks from `start_step` to `end_step` for replay, and returns how many
    /// ticks were queued.
    pub fn add_replay_to_queue(
        &mut self,
        start_step: u32,
        end_step: u32,
        force_view: Option<PlayerId>,
    ) -> u32 {
        if start_step > end_step {
            warn!("start_step must be less than or equal to end_step");
            return 0;
        }

        let queue_length = self.pending_replay.len();
        let data = self
            .retained_ticks
            .iter_oldest_first()
            .filter(|tick| (start_step..=end_step).contains(&tick.game_step))
            .map(|tick| (force_view, tick.clone()));
        self.pending_replay.extend(data);
        let queued = (self.pending_replay.len() - queue_length) as u32;
        let requested = u64::from(end_step - start_step) + 1;
        if u64::from(queued) < requested {
            warn!(
                start_step,
                end_step, queued, "only part of the replay is in the history"
            );
        }
        queued
    }

    pub(super) fn pop_replay_tick(&mut self) -> Option<(Option<PlayerId>, ReplayTick)> {
        self.pending_replay.pop_front()
    }
}
