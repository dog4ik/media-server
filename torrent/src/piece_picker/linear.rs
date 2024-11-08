use crate::scheduler::SchedulerPiece;

use super::Priority;

#[derive(Debug, Clone, Default)]
pub struct Linear {
    pieces: Vec<Piece>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct Piece {
    priority: Priority,
    index: usize,
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
        self.index.cmp(&other.index)
    }
}

impl Linear {
    pub fn new(table: &Vec<SchedulerPiece>) -> Self {
        let mut pieces: Vec<Piece> = table
            .iter()
            .enumerate()
            .filter_map(|(index, p)| {
                if (p.priority.is_disabled() || p.is_finished || p.is_saving)
                    && p.pending_blocks.is_none()
                {
                    None
                } else {
                    Some(Piece {
                        priority: p.priority,
                        index,
                    })
                }
            })
            .collect();
        pieces.sort_by_key(|p| p.priority);
        Self { pieces }
    }

    pub fn build(table: &Vec<SchedulerPiece>) -> Vec<usize> {
        let mut pieces = Vec::new();
        let mut extend_with_priority = |priority: Priority| {
            pieces.extend(table.iter().enumerate().filter_map(|(index, p)| {
                (priority == p.priority
                    && !p.is_finished
                    && !p.is_saving
                    && p.pending_blocks.is_none())
                .then_some(index)
            }));
        };
        extend_with_priority(Priority::Low);
        extend_with_priority(Priority::Medium);
        extend_with_priority(Priority::High);
        pieces
    }

    pub fn next(&mut self) -> Option<usize> {
        self.pieces.pop().map(|p| p.index)
    }

    pub fn peek(&self) -> Option<usize> {
        self.pieces.last().map(|p| p.index)
    }

    pub fn queue_len(&self) -> usize {
        self.pieces.len()
    }

    pub fn queue(&self) -> impl IntoIterator<Item = usize> + '_ {
        self.pieces.iter().rev().map(|p| p.index)
    }

    pub fn put_back(&mut self, index: usize, piece: &SchedulerPiece) {
        let piece = Piece {
            priority: piece.priority,
            index,
        };
        match self.pieces.binary_search(&piece) {
            Ok(_) => {}
            Err(idx) => self.pieces.insert(idx, piece),
        }
    }
}
