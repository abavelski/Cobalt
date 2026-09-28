//! Platform-neutral chessboard state.
//!
//! This is intentionally not a chess engine. It models a physical board:
//! select a piece, then put it on any square. There are no turns or rules.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Color {
    White,
    Black,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PieceKind {
    Pawn,
    Knight,
    Bishop,
    Rook,
    Queen,
    King,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Piece {
    pub color: Color,
    pub kind: PieceKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TapResult {
    NoChange,
    SelectionChanged,
    Moved {
        from: usize,
        to: usize,
    },
    Promotion {
        from: usize,
        to: usize,
        color: Color,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Board {
    squares: [Option<Piece>; 64],
    starting_squares: [Option<Piece>; 64],
    selected: Option<usize>,
}

impl Default for Board {
    fn default() -> Self {
        Self::starting_position()
    }
}

impl Board {
    pub fn starting_position() -> Self {
        let mut squares = [None; 64];
        let back_rank = [
            PieceKind::Rook,
            PieceKind::Knight,
            PieceKind::Bishop,
            PieceKind::Queen,
            PieceKind::King,
            PieceKind::Bishop,
            PieceKind::Knight,
            PieceKind::Rook,
        ];

        // Index 0 is a8, index 63 is h1.
        for (file, kind) in back_rank.into_iter().enumerate() {
            squares[file] = Some(Piece {
                color: Color::Black,
                kind,
            });
            squares[8 + file] = Some(Piece {
                color: Color::Black,
                kind: PieceKind::Pawn,
            });
            squares[48 + file] = Some(Piece {
                color: Color::White,
                kind: PieceKind::Pawn,
            });
            squares[56 + file] = Some(Piece {
                color: Color::White,
                kind,
            });
        }

        Self {
            squares,
            starting_squares: squares,
            selected: None,
        }
    }

    /// Build a board from a standard six-field FEN position.
    ///
    /// The app does not enforce chess move rules, so the FEN metadata is
    /// validated but only the piece placement is used for drawing.
    pub fn from_fen(fen: &str) -> Result<Self, &'static str> {
        let fields: Vec<_> = fen.split_ascii_whitespace().collect();
        if fields.len() != 6 {
            return Err("FEN must contain six fields");
        }

        let ranks: Vec<_> = fields[0].split('/').collect();
        if ranks.len() != 8 {
            return Err("FEN piece placement must contain eight ranks");
        }

        let mut squares = [None; 64];
        for (rank_index, rank) in ranks.iter().enumerate() {
            let mut file = 0usize;
            for symbol in rank.chars() {
                if ('1'..='8').contains(&symbol) {
                    file += symbol.to_digit(10).expect("ASCII digit") as usize;
                    if file > 8 {
                        return Err("FEN rank contains more than eight squares");
                    }
                    continue;
                }

                let piece = match symbol {
                    'P' => Piece {
                        color: Color::White,
                        kind: PieceKind::Pawn,
                    },
                    'N' => Piece {
                        color: Color::White,
                        kind: PieceKind::Knight,
                    },
                    'B' => Piece {
                        color: Color::White,
                        kind: PieceKind::Bishop,
                    },
                    'R' => Piece {
                        color: Color::White,
                        kind: PieceKind::Rook,
                    },
                    'Q' => Piece {
                        color: Color::White,
                        kind: PieceKind::Queen,
                    },
                    'K' => Piece {
                        color: Color::White,
                        kind: PieceKind::King,
                    },
                    'p' => Piece {
                        color: Color::Black,
                        kind: PieceKind::Pawn,
                    },
                    'n' => Piece {
                        color: Color::Black,
                        kind: PieceKind::Knight,
                    },
                    'b' => Piece {
                        color: Color::Black,
                        kind: PieceKind::Bishop,
                    },
                    'r' => Piece {
                        color: Color::Black,
                        kind: PieceKind::Rook,
                    },
                    'q' => Piece {
                        color: Color::Black,
                        kind: PieceKind::Queen,
                    },
                    'k' => Piece {
                        color: Color::Black,
                        kind: PieceKind::King,
                    },
                    _ => return Err("FEN contains an unknown piece symbol"),
                };
                if file >= 8 {
                    return Err("FEN rank contains more than eight squares");
                }
                squares[rank_index * 8 + file] = Some(piece);
                file += 1;
            }
            if file != 8 {
                return Err("each FEN rank must describe exactly eight squares");
            }
        }

        if !matches!(fields[1], "w" | "b") {
            return Err("FEN active color must be w or b");
        }
        if fields[2] != "-" {
            let mut seen = String::new();
            for right in fields[2].chars() {
                if !matches!(right, 'K' | 'Q' | 'k' | 'q') || seen.contains(right) {
                    return Err("FEN castling rights are invalid");
                }
                seen.push(right);
            }
        }
        if fields[3] != "-" {
            let bytes = fields[3].as_bytes();
            let expected_rank = if fields[1] == "w" { b'6' } else { b'3' };
            if bytes.len() != 2 || !(b'a'..=b'h').contains(&bytes[0]) || bytes[1] != expected_rank {
                return Err("FEN en passant square is invalid for the active color");
            }
        }
        fields[4]
            .parse::<u32>()
            .map_err(|_| "FEN halfmove clock must be a nonnegative integer")?;
        let fullmove = fields[5]
            .parse::<u32>()
            .map_err(|_| "FEN fullmove number must be a positive integer")?;
        if fullmove == 0 {
            return Err("FEN fullmove number must be a positive integer");
        }

        Ok(Self {
            squares,
            starting_squares: squares,
            selected: None,
        })
    }

    pub fn reset(&mut self) {
        self.squares = self.starting_squares;
        self.selected = None;
    }

    pub const fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    pub fn piece_at(&self, square: usize) -> Option<Piece> {
        self.squares.get(square).copied().flatten()
    }

    /// Move a piece directly without applying chess legality.
    ///
    /// This is used for stored puzzle replies as well as by the tap state
    /// machine. Moving onto an occupied square replaces the existing piece.
    pub fn move_piece(&mut self, from: usize, to: usize) -> bool {
        if from >= self.squares.len() || to >= self.squares.len() || from == to {
            return false;
        }
        let Some(piece) = self.squares[from].take() else {
            return false;
        };
        self.squares[to] = Some(piece);
        self.selected = None;
        true
    }

    /// Promote a pawn atomically without applying other chess legality.
    pub fn promote_pawn(&mut self, from: usize, to: usize, kind: PieceKind) -> bool {
        if from >= self.squares.len()
            || to >= self.squares.len()
            || from == to
            || matches!(kind, PieceKind::Pawn | PieceKind::King)
        {
            return false;
        }

        let Some(piece) = self.squares[from] else {
            return false;
        };
        if piece.kind != PieceKind::Pawn || !is_promotion_square(piece.color, to) {
            return false;
        }

        self.squares[from] = None;
        self.squares[to] = Some(Piece {
            color: piece.color,
            kind,
        });
        self.selected = None;
        true
    }

    /// Apply one tap and report whether it only changed selection, completed a
    /// move, or staged a promotion that still needs a piece choice.
    ///
    /// - Tap an occupied square to select it.
    /// - Tap it again to deselect it.
    /// - Tap any other square to move the selected piece there.
    /// - A pawn reaching its last rank is not moved until promotion is chosen.
    /// - A move may replace another piece, just like picking pieces up by hand.
    pub fn tap(&mut self, square: usize) -> TapResult {
        if square >= self.squares.len() {
            return TapResult::NoChange;
        }

        match self.selected {
            None => {
                if self.squares[square].is_some() {
                    self.selected = Some(square);
                    TapResult::SelectionChanged
                } else {
                    TapResult::NoChange
                }
            }
            Some(from) if from == square => {
                self.selected = None;
                TapResult::SelectionChanged
            }
            Some(from) => {
                let Some(piece) = self.squares[from] else {
                    // Defensive recovery: selection should always contain a piece.
                    self.selected = None;
                    return TapResult::SelectionChanged;
                };
                if piece.kind == PieceKind::Pawn && is_promotion_square(piece.color, square) {
                    self.selected = None;
                    return TapResult::Promotion {
                        from,
                        to: square,
                        color: piece.color,
                    };
                }
                if self.move_piece(from, square) {
                    TapResult::Moved { from, to: square }
                } else {
                    self.selected = None;
                    TapResult::SelectionChanged
                }
            }
        }
    }
}

const fn is_promotion_square(color: Color, square: usize) -> bool {
    match color {
        Color::White => square < 8,
        Color::Black => square >= 56 && square < 64,
    }
}

#[cfg(test)]
mod tests {
    use super::{Board, Color, Piece, PieceKind, TapResult};

    #[test]
    fn a_piece_can_be_selected_and_moved_anywhere() {
        let mut board = Board::default();

        assert_eq!(board.tap(52), TapResult::SelectionChanged); // e2
        assert_eq!(board.selected(), Some(52));

        assert_eq!(board.tap(36), TapResult::Moved { from: 52, to: 36 }); // e4
        assert!(board.piece_at(52).is_none());
        assert_eq!(
            board.piece_at(36),
            Some(Piece {
                color: Color::White,
                kind: PieceKind::Pawn,
            })
        );
        assert_eq!(board.selected(), None);
    }

    #[test]
    fn tapping_empty_square_first_does_nothing() {
        let mut board = Board::default();
        assert_eq!(board.tap(32), TapResult::NoChange);
        assert_eq!(board.selected(), None);
    }

    #[test]
    fn tapping_selected_piece_again_only_changes_selection() {
        let mut board = Board::default();

        assert_eq!(board.tap(57), TapResult::SelectionChanged); // b1 knight
        assert_eq!(board.tap(57), TapResult::SelectionChanged);

        assert_eq!(board.selected(), None);
        assert_eq!(
            board.piece_at(57),
            Some(Piece {
                color: Color::White,
                kind: PieceKind::Knight,
            })
        );
    }

    #[test]
    fn moving_onto_an_occupied_square_replaces_that_piece() {
        let mut board = Board::default();

        assert_eq!(board.tap(56), TapResult::SelectionChanged); // a1 white rook
        assert_eq!(board.tap(0), TapResult::Moved { from: 56, to: 0 }); // a8 black rook

        assert_eq!(
            board.piece_at(0),
            Some(Piece {
                color: Color::White,
                kind: PieceKind::Rook,
            })
        );
        assert!(board.piece_at(56).is_none());
    }

    #[test]
    fn a_piece_can_be_moved_directly_for_stored_replies() {
        let mut board = Board::default();

        assert!(board.move_piece(13, 5)); // f7 -> f8
        assert!(board.piece_at(13).is_none());
        assert_eq!(
            board.piece_at(5),
            Some(Piece {
                color: Color::Black,
                kind: PieceKind::Pawn,
            })
        );
    }

    #[test]
    fn white_and_black_promotion_are_staged_without_moving_the_pawn() {
        let mut white = Board::from_fen("7k/P7/8/8/8/8/8/K7 w - - 0 1").unwrap();
        assert_eq!(white.tap(8), TapResult::SelectionChanged); // a7
        assert_eq!(
            white.tap(0),
            TapResult::Promotion {
                from: 8,
                to: 0,
                color: Color::White,
            }
        );
        assert_eq!(
            white.piece_at(8),
            Some(Piece {
                color: Color::White,
                kind: PieceKind::Pawn,
            })
        );
        assert!(white.piece_at(0).is_none());

        let mut black = Board::from_fen("k7/8/8/8/8/8/p7/7K b - - 0 1").unwrap();
        assert_eq!(black.tap(48), TapResult::SelectionChanged); // a2
        assert_eq!(
            black.tap(56),
            TapResult::Promotion {
                from: 48,
                to: 56,
                color: Color::Black,
            }
        );
        assert_eq!(
            black.piece_at(48),
            Some(Piece {
                color: Color::Black,
                kind: PieceKind::Pawn,
            })
        );
        assert!(black.piece_at(56).is_none());
    }

    #[test]
    fn non_pawn_move_to_last_rank_does_not_trigger_promotion() {
        let mut board = Board::from_fen("7k/R7/8/8/8/8/8/K7 w - - 0 1").unwrap();

        assert_eq!(board.tap(8), TapResult::SelectionChanged);
        assert_eq!(board.tap(0), TapResult::Moved { from: 8, to: 0 });
        assert_eq!(
            board.piece_at(0),
            Some(Piece {
                color: Color::White,
                kind: PieceKind::Rook,
            })
        );
    }

    #[test]
    fn promote_pawn_supports_all_standard_pieces() {
        for kind in [
            PieceKind::Queen,
            PieceKind::Rook,
            PieceKind::Bishop,
            PieceKind::Knight,
        ] {
            let mut board = Board::from_fen("7k/P7/8/8/8/8/8/K7 w - - 0 1").unwrap();
            assert!(board.promote_pawn(8, 0, kind));
            assert!(board.piece_at(8).is_none());
            assert_eq!(
                board.piece_at(0),
                Some(Piece {
                    color: Color::White,
                    kind,
                })
            );
        }
    }

    #[test]
    fn promotion_rejects_invalid_piece_or_destination() {
        let mut board = Board::from_fen("7k/P7/8/8/8/8/8/K7 w - - 0 1").unwrap();

        assert!(!board.promote_pawn(8, 16, PieceKind::Queen));
        assert!(!board.promote_pawn(8, 0, PieceKind::King));
        assert!(!board.promote_pawn(8, 0, PieceKind::Pawn));
    }

    #[test]
    fn reset_restores_the_initial_position() {
        let mut board = Board::default();
        board.tap(52);
        board.tap(36);
        board.reset();

        assert_eq!(board, Board::starting_position());
    }
}
