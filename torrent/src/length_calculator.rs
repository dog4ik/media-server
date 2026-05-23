/// Computes piece lengths for a torrent, accounting for the last piece.
#[derive(Debug, Clone)]
pub struct LengthCalculator {
    pub total_pieces: usize,
    /// Default length of a single piece
    pub piece_length: u32,
    /// Length of the last piece
    pub last_length: u32,
}

impl LengthCalculator {
    pub const fn new(total_size: u64, piece_length: u32) -> Self {
        let total_pieces = total_size.div_ceil(piece_length as u64) as usize;
        let remainder = (total_size % piece_length as u64) as u32;
        // When total_size is an exact multiple of piece_length, the last piece has the same size as every other piece.
        let last_length = if remainder == 0 {
            piece_length
        } else {
            remainder
        };

        Self {
            total_pieces,
            piece_length,
            last_length,
        }
    }

    /// Length of the piece, with consideration of the last piece
    pub fn piece_length(&self, piece_i: usize) -> u32 {
        if piece_i == self.total_pieces - 1 {
            self.last_length
        } else {
            self.piece_length
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LengthCalculator;

    #[test]
    fn exact_multiple_last_piece_is_full_size() {
        // 4 pieces × 1 MiB: last piece must NOT be 0
        let c = LengthCalculator::new(4 * 1024 * 1024, 1024 * 1024);
        assert_eq!(c.total_pieces, 4);
        assert_eq!(c.piece_length(3), 1024 * 1024);
    }

    #[test]
    fn non_multiple_last_piece_is_remainder() {
        // 3.5 MiB with 1 MiB pieces → last piece = 512 KiB
        let c = LengthCalculator::new(3 * 1024 * 1024 + 512 * 1024, 1024 * 1024);
        assert_eq!(c.total_pieces, 4);
        assert_eq!(c.piece_length(3), 512 * 1024);
    }

    #[test]
    fn middle_pieces_always_full_size() {
        let piece_len = 512 * 1024u32;
        let c = LengthCalculator::new(10 * piece_len as u64 + 1, piece_len);
        for i in 0..c.total_pieces - 1 {
            assert_eq!(c.piece_length(i), piece_len, "piece {i} should be full");
        }
    }

    #[test]
    fn single_piece_exact() {
        let c = LengthCalculator::new(256 * 1024, 256 * 1024);
        assert_eq!(c.total_pieces, 1);
        assert_eq!(c.piece_length(0), 256 * 1024);
    }

    #[test]
    fn single_piece_smaller_than_piece_length() {
        let c = LengthCalculator::new(1000, 256 * 1024);
        assert_eq!(c.total_pieces, 1);
        assert_eq!(c.piece_length(0), 1000);
    }

    #[test]
    fn sum_of_all_pieces_equals_total_size() {
        let total_size = 100_000_007u64;
        let piece_len = 512 * 1024u32;
        let c = LengthCalculator::new(total_size, piece_len);
        let sum: u64 = (0..c.total_pieces).map(|i| c.piece_length(i) as u64).sum();
        assert_eq!(sum, total_size);
    }

    #[test]
    fn sum_of_all_pieces_exact_multiple() {
        let piece_len = 1024 * 1024u32;
        let total_size = 418 * piece_len as u64; // exact multiple like the youtube.torrent bug
        let c = LengthCalculator::new(total_size, piece_len);
        assert_eq!(c.total_pieces, 418);
        let sum: u64 = (0..c.total_pieces).map(|i| c.piece_length(i) as u64).sum();
        assert_eq!(sum, total_size);
    }
}
