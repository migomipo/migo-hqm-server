//! Packet sequencing and history used to replicate game state to clients.

use std::time::Instant;

use arraydeque::{ArrayDeque, Wrapping};

use crate::game::ScoreboardValues;
use crate::protocol::ObjectPacket;

pub(super) const PACKET_HISTORY_LEN: usize = 192;
const NO_PACKET_SEQUENCE: u32 = u32::MAX;

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
        let next = self.0.wrapping_add(1);
        if next == NO_PACKET_SEQUENCE {
            Self(0)
        } else {
            Self(next)
        }
    }

    pub(crate) fn is_newer_than(self, other: Self) -> bool {
        let distance = self.0.wrapping_sub(other.0);
        distance != 0 && distance < u32::MAX / 2
    }
}

struct ReplicationFrame {
    sequence: PacketNumber,
    objects: [ObjectPacket; 32],
    created_at: Instant,
}

pub(crate) struct PacketHistory {
    latest: Option<PacketNumber>,
    frames: Box<ArrayDeque<ReplicationFrame, PACKET_HISTORY_LEN, Wrapping>>,
}

impl PacketHistory {
    pub(crate) fn new() -> Self {
        Self {
            latest: None,
            frames: Box::new(ArrayDeque::new()),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.latest = None;
        self.frames.clear();
    }

    pub(crate) fn push(&mut self, objects: [ObjectPacket; 32]) -> PacketNumber {
        let sequence = self
            .latest
            .map(PacketNumber::next)
            .unwrap_or(PacketNumber(0));
        self.frames.push_front(ReplicationFrame {
            sequence,
            objects,
            created_at: Instant::now(),
        });
        self.latest = Some(sequence);
        sequence
    }

    pub(crate) fn latest_sequence(&self) -> PacketNumber {
        self.latest
            .expect("packet history must have a frame before serialization")
    }

    pub(crate) fn current_objects(&self) -> &[ObjectPacket; 32] {
        &self
            .frames
            .front()
            .expect("packet history must have a frame before serialization")
            .objects
    }

    fn frame(&self, sequence: PacketNumber) -> Option<&ReplicationFrame> {
        let latest = self.latest?;
        let mut age = latest.0.wrapping_sub(sequence.0);
        // Numbering skips NO_PACKET_SEQUENCE, so frames from before a wraparound are one step newer
        if sequence.0 > latest.0 {
            age = age.wrapping_sub(1);
        }
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
        if self.latest == Some(sequence) {
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
