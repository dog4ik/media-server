use std::fmt::Display;

use linear::Linear;

use crate::{peers::BitField, scheduler::SchedulerPiece};

mod linear;
mod rare_first;

#[derive(Debug, Clone)]
pub struct PiecePicker {
    strategy: ScheduleStrategy,
    queue: Vec<usize>,
}

impl PiecePicker {
    pub fn new(piece_table: &Vec<SchedulerPiece>) -> Self {
        let mut this = Self {
            strategy: ScheduleStrategy::default(),
            queue: Vec::new(),
        };
        this.rebuild_queue(piece_table);
        this
    }

    pub fn pop_closest_for_bitfield(&mut self, bf: &BitField) -> Option<usize> {
        self.queue.iter().rev().position(|p| bf.has(*p)).map(|pos| {
            let idx = self.queue.len() - pos;
            self.queue.remove(idx)
        })
    }

    pub fn peek_next(&self) -> Option<usize> {
        self.queue.last().copied()
    }

    /// Pop next rational piece
    pub fn pop_next(&mut self) -> Option<usize> {
        self.queue.pop()
    }

    pub fn rebuild_queue(&mut self, piece_table: &Vec<SchedulerPiece>) {
        self.queue = self.strategy.build(piece_table);
    }

    pub fn put_back(&mut self, index: usize) {
        self.queue.push(index);
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn strategy(&self) -> ScheduleStrategy {
        self.strategy
    }

    pub fn set_strategy(&mut self, strategy: ScheduleStrategy) {
        self.strategy = strategy;
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum ScheduleStrategy {
    #[default]
    Linear,
    RareFirst,
    Request(usize),
}

impl ScheduleStrategy {
    pub fn build(&self, piece_table: &Vec<SchedulerPiece>) -> Vec<usize> {
        match self {
            ScheduleStrategy::Linear => Linear::build(piece_table),
            ScheduleStrategy::RareFirst => todo!(),
            ScheduleStrategy::Request(_) => todo!(),
        }
    }
}

impl Display for ScheduleStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScheduleStrategy::Linear => write!(f, "Linear"),
            ScheduleStrategy::RareFirst => write!(f, "Rare first"),
            ScheduleStrategy::Request(piece) => write!(f, "Piece request: {}", piece),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Priority {
    Disabled = 0,
    Low = 1,
    #[default]
    Medium = 2,
    High = 3,
}

impl Priority {
    pub fn is_disabled(&self) -> bool {
        *self == Priority::Disabled
    }
}

impl TryFrom<usize> for Priority {
    type Error = anyhow::Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        let priority = match value {
            0 => Self::Disabled,
            1 => Self::Low,
            2 => Self::Medium,
            3 => Self::High,
            _ => {
                return Err(anyhow::anyhow!(
                    "expected value in range 0..4, got {}",
                    value
                ))
            }
        };
        Ok(priority)
    }
}
