use std::collections::BTreeMap;

use crate::scheduler::SchedulerPiece;

use super::Priority;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Piece {
    rarity: u8,
    priority: Priority,
}

impl PartialOrd for Piece {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Piece {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if self.priority != other.priority {
            return self.priority.cmp(&other.priority);
        }
        self.rarity.cmp(&other.rarity)
    }
}

#[derive(Debug, Clone, Default)]
pub struct RareFirst {
    pieces: BTreeMap<usize, Piece>,
}

impl RareFirst {
    pub fn new(pieces: &Vec<SchedulerPiece>) -> Self {
        let pieces = pieces
            .iter()
            .enumerate()
            .filter_map(|(index, p)| {
                if p.priority.is_disabled() {
                    None
                } else {
                    Some((
                        index,
                        Piece {
                            rarity: p.rarity,
                            priority: p.priority,
                        },
                    ))
                }
            })
            .collect();
        Self { pieces }
    }

    pub fn update_rarity(&mut self, index: usize, new_rarity: u8) -> Option<()> {
        self.pieces.get_mut(&index)?.rarity = new_rarity;
        Some(())
    }

    pub fn next(&mut self) -> Option<usize> {
        self.pieces.pop_last().map(|(k, _)| k)
    }

    pub fn peek(&self) -> Option<usize> {
        self.pieces.last_key_value().map(|(k, _)| *k)
    }

    pub fn put_back(&mut self, index: usize, piece: &SchedulerPiece) {
        let piece = Piece {
            rarity: piece.rarity,
            priority: piece.priority,
        };
        self.pieces.insert(index, piece);
    }
}
