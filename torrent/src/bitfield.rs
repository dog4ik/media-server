use anyhow::Context;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitField(pub Vec<u8>);

impl BitField {
    pub fn new(data: &[u8]) -> Self {
        Self(data.to_vec())
    }

    pub fn has(&self, piece: usize) -> bool {
        let bytes = &self.0;
        let Some(block) = bytes.get(piece / 8) else {
            return false;
        };
        let position = (piece % 8) as u32;

        block & 1u8.rotate_right(position + 1) != 0
    }

    pub fn add(&mut self, piece: usize) -> anyhow::Result<()> {
        let bytes = &mut self.0;
        let Some(block) = bytes.get_mut(piece / 8) else {
            return Err(anyhow::anyhow!("piece {piece} does not exist"));
        };
        let position = (piece % 8) as u32;
        let new_value = *block | 1u8.rotate_right(position + 1);
        *block = new_value;
        Ok(())
    }

    pub fn all_pieces(&self, total_pieces: usize) -> impl IntoIterator<Item = bool> + '_ {
        self.0.iter().enumerate().flat_map(move |(i, byte)| {
            (0..8).filter_map(move |position| {
                let piece_i = i * 8 + (position as usize);
                if piece_i > total_pieces {
                    None
                } else {
                    let mask = 1u8.rotate_right(position + 1);
                    Some(byte & mask != 0)
                }
            })
        })
    }

    pub fn is_full(&self, max_pieces: usize) -> bool {
        if self.0.is_empty() {
            return true;
        }
        let mut pieces = 0;
        for byte in &self.0[..self.0.len() - 1] {
            if *byte != u8::MAX {
                return false;
            }
            pieces += byte.count_ones();
        }
        let last = self.0.last().unwrap();
        pieces += last.count_ones();
        pieces as usize == max_pieces
    }

    pub fn remove(&mut self, piece: usize) -> anyhow::Result<()> {
        let bytes = &mut self.0;
        let Some(block) = bytes.get_mut(piece / 8) else {
            return Err(anyhow::anyhow!("piece {piece} does not exist"));
        };
        let position = (piece % 8) as u32;
        let new_value = *block & !1u8.rotate_right(position + 1);
        *block = new_value;
        Ok(())
    }

    pub fn pieces(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(i, byte)| {
            (0..8).filter_map(move |position| {
                let piece_i = i * 8 + (position as usize);
                let mask = 1u8.rotate_right(position + 1);
                (byte & mask != 0).then_some(piece_i)
            })
        })
    }

    pub fn missing_pieces(&self, total_pieces: usize) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(move |(i, byte)| {
            (0..8).filter_map(move |position| {
                let piece_i = i * 8 + (position as usize);
                if piece_i >= total_pieces {
                    return None;
                }
                let mask = 1u8.rotate_right(position + 1);
                (byte & mask == 0).then_some(piece_i)
            })
        })
    }

    pub fn empty(pieces_amount: usize) -> Self {
        Self(vec![0; std::cmp::max(pieces_amount.div_ceil(8), 1)])
    }

    /// Make sure that bitield is appropriate for given pieces amount.
    /// Fails if there are any 1's after the end or it is small or large to fit given pieces.
    pub fn validate(&self, total_pieces: usize) -> anyhow::Result<()> {
        let bitfield_pieces = self.0.len() * 8;
        let leftover = bitfield_pieces
            .checked_sub(total_pieces)
            .context("bitfield has less capacity than needed")?;
        if leftover >= 8 {
            anyhow::bail!("bitfield is larger than needed")
        }
        for piece in (bitfield_pieces - leftover)..bitfield_pieces {
            anyhow::ensure!(!self.has(piece));
        }
        Ok(())
    }

    /// Perform bitwise | with other
    pub fn or(&mut self, other: &Self) {
        for (self_byte, other_byte) in self.0.iter_mut().zip(&other.0) {
            *self_byte |= other_byte;
        }
    }
}

impl From<Vec<u8>> for BitField {
    fn from(value: Vec<u8>) -> Self {
        BitField(value)
    }
}

#[cfg(test)]
mod test {

    use super::BitField;

    #[test]
    fn bitfield_has() {
        let data = [0b01110101, 0b01110001];
        let bitfield = BitField::new(&data);
        assert!(!bitfield.has(0));
        assert!(bitfield.has(1));
        assert!(bitfield.has(2));
        assert!(bitfield.has(3));
        assert!(!bitfield.has(4));
        assert!(bitfield.has(5));
        assert!(!bitfield.has(6));
        assert!(bitfield.has(7));
        assert!(!bitfield.has(8));
        assert!(bitfield.has(9));
        assert!(bitfield.has(10));
        assert!(bitfield.has(11));
        assert!(!bitfield.has(12));
        assert!(!bitfield.has(13));
        assert!(!bitfield.has(14));
        assert!(bitfield.has(15));
        assert!(!bitfield.has(16));
        assert!(!bitfield.has(17));
    }
    #[test]
    fn bitfield_add() {
        let data = [0b01110101, 0b01110001];
        let mut bitfield = BitField::new(&data);
        bitfield.add(0).unwrap();
        bitfield.add(1).unwrap();
        bitfield.add(4).unwrap();
        bitfield.add(8).unwrap();
        bitfield.add(14).unwrap();
        assert!(bitfield.has(0));
        assert!(bitfield.has(1));
        assert!(bitfield.has(2));
        assert!(bitfield.has(3));
        assert!(bitfield.has(4));
        assert!(bitfield.has(5));
        assert!(!bitfield.has(6));
        assert!(bitfield.has(7));
        assert!(bitfield.has(8));
        assert!(bitfield.has(9));
        assert!(bitfield.has(10));
        assert!(bitfield.has(11));
        assert!(!bitfield.has(12));
        assert!(!bitfield.has(13));
        assert!(bitfield.has(14));
        assert!(bitfield.has(15));
        assert!(!bitfield.has(16));
        assert!(!bitfield.has(17));
        assert!(bitfield.add(16).is_err());
    }

    #[test]
    fn bitfield_remove() {
        let data = [0b01110101, 0b01110001];
        let mut bitfield = BitField::new(&data);
        bitfield.remove(1).unwrap();
        bitfield.remove(4).unwrap();
        bitfield.remove(9).unwrap();
        bitfield.remove(15).unwrap();
        assert!(!bitfield.has(0));
        assert!(!bitfield.has(1));
        assert!(bitfield.has(2));
        assert!(bitfield.has(3));
        assert!(!bitfield.has(4));
        assert!(bitfield.has(5));
        assert!(!bitfield.has(6));
        assert!(bitfield.has(7));
        assert!(!bitfield.has(8));
        assert!(!bitfield.has(9));
        assert!(bitfield.has(10));
        assert!(bitfield.has(11));
        assert!(!bitfield.has(12));
        assert!(!bitfield.has(13));
        assert!(!bitfield.has(14));
        assert!(!bitfield.has(15));
        assert!(!bitfield.has(16));
        assert!(!bitfield.has(17));
        assert!(bitfield.remove(16).is_err());
    }

    #[test]
    fn bitfield_iterator() {
        let data = [0b01110101, 0b01110001];
        let bitfield = BitField::new(&data);
        let mut iterator = bitfield.pieces();
        assert_eq!(Some(1), iterator.next());
        assert_eq!(Some(2), iterator.next());
        assert_eq!(Some(3), iterator.next());
        assert_eq!(Some(5), iterator.next());
        assert_eq!(Some(7), iterator.next());
        assert_eq!(Some(9), iterator.next());
        assert_eq!(Some(10), iterator.next());
        assert_eq!(Some(11), iterator.next());
        assert_eq!(Some(15), iterator.next());
        assert_eq!(None, iterator.next());
    }

    #[test]
    fn bitfield_validate() {
        let data = [0b01110101, 0b01110001, 0b00100000];
        let bitfield = BitField::new(&data);
        assert!(bitfield.validate(16).is_err());
        assert!(bitfield.validate(1).is_err());
        assert!(bitfield.validate(13).is_err());
        assert!(bitfield.validate(18).is_err());
        assert!(bitfield.validate(19).is_ok());
        assert!(bitfield.validate(20).is_ok());
        assert!(bitfield.validate(24).is_ok());
        assert!(bitfield.validate(25).is_err());
        assert!(bitfield.validate(100).is_err());
        let data = [0b01110100];
        let bitfield = BitField::new(&data);
        assert!(bitfield.validate(1).is_err());
        assert!(bitfield.validate(4).is_err());
        assert!(bitfield.validate(5).is_err());
        assert!(bitfield.validate(6).is_ok());
        assert!(bitfield.validate(7).is_ok());
        assert!(bitfield.validate(8).is_ok());
        assert!(bitfield.validate(9).is_err());
        assert!(bitfield.validate(100).is_err());
        let data = [0b11111111, 0b00000000];
        let bitfield = BitField::new(&data);
        assert!(bitfield.validate(1).is_err());
        assert!(bitfield.validate(4).is_err());
        assert!(bitfield.validate(5).is_err());
        assert!(bitfield.validate(6).is_err());
        assert!(bitfield.validate(7).is_err());
        assert!(bitfield.validate(8).is_err());
        assert!(bitfield.validate(9).is_ok());
        assert!(bitfield.validate(100).is_err());
    }
}
