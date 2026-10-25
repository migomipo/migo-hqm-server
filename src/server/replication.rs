//! Packet sequencing and history used to replicate game state to clients.

use std::time::Instant;

use crate::game::ScoreboardValues;
use crate::history::History;
use crate::protocol::ObjectPacket;

pub(super) const PACKET_HISTORY_LEN: usize = 192;
const NO_PACKET_SEQUENCE: u32 = u32::MAX;
/// Packet numbers use every `u32` except [NO_PACKET_SEQUENCE], so they count modulo this.
const PACKET_NUMBER_COUNT: u64 = NO_PACKET_SEQUENCE as u64;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct PacketNumber(pub(crate) u32);

impl PacketNumber {
    pub(crate) fn from_wire(value: u32) -> Option<Self> {
        (value != NO_PACKET_SEQUENCE).then_some(Self(value))
    }

    pub(crate) fn to_wire(self) -> u32 {
        self.0
    }

    pub(crate) fn next(self) -> Self {
        Self(((u64::from(self.0) + 1) % PACKET_NUMBER_COUNT) as u32)
    }

    /// How many packets were sent after `older` up to and including this one.
    pub(crate) fn steps_since(self, older: Self) -> u32 {
        ((u64::from(self.0) + PACKET_NUMBER_COUNT - u64::from(older.0)) % PACKET_NUMBER_COUNT)
            as u32
    }

    pub(crate) fn is_newer_than(self, other: Self) -> bool {
        let distance = self.steps_since(other);
        distance != 0 && distance < NO_PACKET_SEQUENCE / 2
    }
}

struct ReplicationFrame {
    sequence: PacketNumber,
    objects: [ObjectPacket; 32],
    created_at: Instant,
}

pub(crate) struct PacketHistory {
    frames: History<ReplicationFrame>,
}

impl PacketHistory {
    pub(crate) fn new() -> Self {
        Self {
            frames: History::new(PACKET_HISTORY_LEN),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.frames.clear();
    }

    pub(crate) fn push(&mut self, objects: [ObjectPacket; 32]) -> PacketNumber {
        let sequence = self.latest().map_or(PacketNumber(0), PacketNumber::next);
        self.frames.push(ReplicationFrame {
            sequence,
            objects,
            created_at: Instant::now(),
        });
        sequence
    }

    fn latest(&self) -> Option<PacketNumber> {
        self.frames.newest().map(|frame| frame.sequence)
    }

    pub(crate) fn latest_sequence(&self) -> PacketNumber {
        self.latest()
            .expect("packet history must have a frame before serialization")
    }

    pub(crate) fn current_objects(&self) -> &[ObjectPacket; 32] {
        &self
            .frames
            .newest()
            .expect("packet history must have a frame before serialization")
            .objects
    }

    fn frame(&self, sequence: PacketNumber) -> Option<&ReplicationFrame> {
        let age = self.latest()?.steps_since(sequence);
        self.frames
            .get(age as usize)
            .filter(|frame| frame.sequence == sequence)
    }

    pub(crate) fn objects_for(&self, sequence: PacketNumber) -> Option<&[ObjectPacket; 32]> {
        self.frame(sequence).map(|frame| &frame.objects)
    }

    pub(crate) fn baseline_objects_for(
        &self,
        sequence: PacketNumber,
    ) -> Option<&[ObjectPacket; 32]> {
        if self.latest() == Some(sequence) {
            None
        } else {
            self.objects_for(sequence)
        }
    }

    pub(crate) fn sent_at(&self, sequence: PacketNumber) -> Option<Instant> {
        self.frame(sequence).map(|frame| frame.created_at)
    }
}

pub(super) struct ReplicationState {
    pub(super) last_scoreboard: Option<ScoreboardValues>,
    pub(super) history: PacketHistory,
}

impl ReplicationState {
    pub(super) fn new() -> Self {
        Self {
            last_scoreboard: None,
            history: PacketHistory::new(),
        }
    }

    pub(super) fn new_game(&mut self) {
        self.last_scoreboard = None;
        self.history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PuckPacket;

    fn objects(marker: u32) -> [ObjectPacket; 32] {
        let mut objects = std::array::from_fn(|_| ObjectPacket::None);
        objects[0] = ObjectPacket::Puck(PuckPacket {
            pos: (marker, 0, 0),
            rot: (0, 0),
        });
        objects
    }

    #[test]
    fn packet_numbers_skip_the_no_packet_marker() {
        assert_eq!(PacketNumber(5).next(), PacketNumber(6));
        assert_eq!(PacketNumber(u32::MAX - 1).next(), PacketNumber(0));
        assert_eq!(PacketNumber::from_wire(u32::MAX), None);
        assert_eq!(PacketNumber::from_wire(0), Some(PacketNumber(0)));
        assert_eq!(PacketNumber(1234).to_wire(), 1234);
    }

    #[test]
    fn newer_packet_numbers_compare_across_wraparound() {
        assert!(PacketNumber(6).is_newer_than(PacketNumber(5)));
        assert!(!PacketNumber(5).is_newer_than(PacketNumber(6)));
        assert!(!PacketNumber(5).is_newer_than(PacketNumber(5)));

        let before_wrap = PacketNumber(u32::MAX - 1);
        let after_wrap = before_wrap.next();
        assert!(after_wrap.is_newer_than(before_wrap));
        assert!(!before_wrap.is_newer_than(after_wrap));
    }

    #[test]
    fn steps_between_packet_numbers_count_across_wraparound() {
        assert_eq!(PacketNumber(6).steps_since(PacketNumber(5)), 1);
        assert_eq!(PacketNumber(5).steps_since(PacketNumber(5)), 0);
        assert_eq!(PacketNumber(0).steps_since(PacketNumber(u32::MAX - 1)), 1);
        assert_eq!(PacketNumber(1).steps_since(PacketNumber(u32::MAX - 2)), 3);
        assert_eq!(PacketNumber(5).steps_since(PacketNumber(6)), u32::MAX - 1);
    }

    #[test]
    fn history_numbers_frames_from_zero_and_restarts_after_clear() {
        let mut history = PacketHistory::new();
        assert_eq!(history.push(objects(0)), PacketNumber(0));
        assert_eq!(history.push(objects(1)), PacketNumber(1));
        assert_eq!(history.latest_sequence(), PacketNumber(1));
        assert_eq!(history.current_objects(), &objects(1));

        history.clear();

        assert_eq!(history.objects_for(PacketNumber(0)), None);
        assert_eq!(history.push(objects(2)), PacketNumber(0));
    }

    #[test]
    fn history_looks_up_recent_frames() {
        let mut history = PacketHistory::new();
        for marker in 0..5 {
            history.push(objects(marker));
        }

        assert_eq!(history.objects_for(PacketNumber(2)), Some(&objects(2)));
        assert!(history.sent_at(PacketNumber(2)).is_some());
        assert_eq!(history.objects_for(PacketNumber(5)), None);
        assert_eq!(history.sent_at(PacketNumber(5)), None);
    }

    #[test]
    fn history_forgets_frames_older_than_its_length() {
        let mut history = PacketHistory::new();
        for marker in 0..=PACKET_HISTORY_LEN as u32 {
            history.push(objects(marker));
        }

        assert_eq!(history.objects_for(PacketNumber(0)), None);
        assert_eq!(history.sent_at(PacketNumber(0)), None);
        assert_eq!(history.objects_for(PacketNumber(1)), Some(&objects(1)));
    }

    #[test]
    fn the_latest_frame_is_no_baseline() {
        let mut history = PacketHistory::new();
        let previous = history.push(objects(0));
        let latest = history.push(objects(1));

        assert_eq!(history.baseline_objects_for(latest), None);
        assert_eq!(history.baseline_objects_for(previous), Some(&objects(0)));
    }

    #[test]
    fn history_looks_up_frames_across_wraparound() {
        let mut history = PacketHistory::new();
        history.frames.push(ReplicationFrame {
            sequence: PacketNumber(u32::MAX - 3),
            objects: objects(99),
            created_at: Instant::now(),
        });
        let pushed: Vec<_> = (0..4)
            .map(|marker| (history.push(objects(marker)), marker))
            .collect();
        assert_eq!(
            pushed
                .iter()
                .map(|(sequence, _)| sequence.0)
                .collect::<Vec<_>>(),
            [u32::MAX - 2, u32::MAX - 1, 0, 1]
        );

        for (sequence, marker) in pushed {
            assert_eq!(
                history.objects_for(sequence),
                Some(&objects(marker)),
                "{sequence:?}"
            );
            assert!(history.sent_at(sequence).is_some(), "{sequence:?}");
        }
    }
}
