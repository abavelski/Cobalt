mod board;
mod puzzle;

use board::{Board, Color, Piece, PieceKind};
use kobo_sdk::{
    action_id, ActionId, BandAlign, Context, Glyph, KoboApp, Screen, ScreenBuilder, SlotWidth,
    StoreResult,
};
use puzzle::{parse_puzzle_file, Puzzle};
use std::process::ExitCode;

const SIDE: usize = 8;
const CELLS: usize = SIDE * SIDE;
const RESET: &str = "reset";
const FLIP: &str = "flip";
const EXIT: &str = "exit";
const PUZZLES_FILE: &str = "puzzles.json";
const EXAMPLE_PUZZLES: &[u8] = include_bytes!("puzzles.json");

#[derive(Default)]
struct ChessBoardApp {
    board: Board,
    puzzles: Vec<Puzzle>,
    puzzle_index: usize,
    file_error: Option<String>,
    sleeping: bool,
    flipped: bool,
}

impl ChessBoardApp {
    fn activate_puzzles(&mut self, context: &mut Context, puzzles: Vec<Puzzle>, save_copy: bool) {
        self.puzzles = puzzles;
        self.select_puzzle(0);
        if save_copy {
            context.store().save(PUZZLES_FILE, EXAMPLE_PUZZLES.to_vec());
        }
        self.show(context);
    }

    fn load_examples(&mut self, context: &mut Context, save_copy: bool) {
        let puzzles =
            parse_puzzle_file(EXAMPLE_PUZZLES).expect("bundled example puzzles are valid");
        self.activate_puzzles(context, puzzles, save_copy);
    }

    fn select_puzzle(&mut self, index: usize) {
        let puzzle = &self.puzzles[index];
        self.board = Board::from_fen(&puzzle.fen).expect("validated puzzle FEN");
        self.flipped = puzzle.side_to_move() == Color::Black;
        self.puzzle_index = index;
    }

    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }

    fn screen(&self) -> Screen {
        let selected = self.board.selected();

        let cells = (0..CELLS).map(|display_square| {
            let square = board_square(display_square, self.flipped);
            let piece = self.board.piece_at(square);
            (
                square_action(square),
                square_label(square, piece),
                piece.map(piece_glyph),
                selected == Some(square),
            )
        });

        let title = if self.sleeping {
            "Sleeping - press power to wake".to_owned()
        } else if self.file_error.is_some() {
            "Puzzle file needs fixing".to_owned()
        } else {
            format!(
                "E-Ink Chess {}/{}",
                self.puzzle_index + 1,
                self.puzzles.len()
            )
        };
        let mut screen = ScreenBuilder::new("chessboard")
            .top_bar(title)
            .top_bar_glyph(EXIT, "Return to Kobo reader", Glyph::Close)
            .board_with_selection(SIDE as u8, cells)
            .band(
                BandAlign::Middle,
                [
                    (
                        SlotWidth::Fill,
                        (|slot: ScreenBuilder| slot) as fn(ScreenBuilder) -> ScreenBuilder,
                    ),
                    (
                        // Two 10 mm targets with the grid's 1 mm gap.
                        SlotWidth::Fixed(210),
                        |slot| {
                            slot.controls(
                                2,
                                [
                                    (RESET, "Reset position", Glyph::Refresh),
                                    (FLIP, "Flip board", Glyph::SwapVertical),
                                ],
                            )
                        },
                    ),
                ],
            );
        if let Some(puzzle) = self.puzzles.get(self.puzzle_index) {
            screen = screen.secondary(match puzzle.side_to_move() {
                Color::White => "White to move",
                Color::Black => "Black to move",
            });
            if let Some(description) = puzzle
                .description
                .as_deref()
                .filter(|text| !text.trim().is_empty())
            {
                screen = screen.text(description);
            }
        }
        if let Some(error) = self.file_error.as_ref() {
            screen = screen.text(error.clone());
        }
        screen.build()
    }

    fn turn_puzzle(&mut self, context: &mut Context, forward: bool) {
        let next = if forward {
            self.puzzle_index
                .checked_add(1)
                .filter(|&index| index < self.puzzles.len())
        } else {
            self.puzzle_index.checked_sub(1)
        };
        let Some(next) = next else {
            return;
        };
        self.select_puzzle(next);
        self.show(context);
    }
}

impl KoboApp for ChessBoardApp {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(PUZZLES_FILE);
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key != PUZZLES_FILE {
            return;
        }
        match result {
            StoreResult::Loaded {
                value: Some(bytes), ..
            } => match parse_puzzle_file(&bytes) {
                Ok(puzzles) => {
                    self.file_error = None;
                    self.activate_puzzles(context, puzzles, false);
                }
                Err(error) => {
                    self.file_error = Some(error);
                    self.load_examples(context, false);
                }
            },
            StoreResult::Loaded { value: None, .. } => {
                self.file_error = None;
                self.load_examples(context, true);
            }
            StoreResult::Denied(_) => {
                self.file_error = Some("puzzles.json could not be read; showing examples.".into());
                self.load_examples(context, false);
            }
            _ => {}
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id(EXIT) {
            context.exit();
            return;
        }

        if action == action_id(RESET) {
            self.board.reset();
            self.show(context);
            return;
        }

        if action == action_id(FLIP) {
            self.flipped = !self.flipped;
            self.show(context);
            return;
        }

        for square in 0..CELLS {
            if action == action_id(&square_action(square)) {
                // Avoid an e-ink refresh when a tap changed nothing.
                if self.board.tap(square) {
                    self.show(context);
                }
                return;
            }
        }
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        self.turn_puzzle(context, forward);
    }

    fn on_suspend(&mut self, context: &mut Context) {
        self.sleeping = true;
        self.show(context);
    }

    fn on_resume(&mut self, context: &mut Context) {
        self.sleeping = false;
        self.show(context);
    }
}

fn square_action(square: usize) -> String {
    format!("square-{square}")
}

const fn board_square(display_square: usize, flipped: bool) -> usize {
    if flipped {
        CELLS - 1 - display_square
    } else {
        display_square
    }
}

fn square_label(square: usize, piece: Option<Piece>) -> String {
    let file = (b'a' + (square % SIDE) as u8) as char;
    let rank = (b'8' - (square / SIDE) as u8) as char;
    match piece {
        Some(piece) => format!("{file}{rank} {}", piece_label(piece)),
        None => format!("{file}{rank}"),
    }
}

const fn piece_label(piece: Piece) -> &'static str {
    match (piece.color, piece.kind) {
        (Color::White, PieceKind::King) => "K",
        (Color::White, PieceKind::Queen) => "Q",
        (Color::White, PieceKind::Rook) => "R",
        (Color::White, PieceKind::Bishop) => "B",
        (Color::White, PieceKind::Knight) => "N",
        (Color::White, PieceKind::Pawn) => "P",
        (Color::Black, PieceKind::King) => "k",
        (Color::Black, PieceKind::Queen) => "q",
        (Color::Black, PieceKind::Rook) => "r",
        (Color::Black, PieceKind::Bishop) => "b",
        (Color::Black, PieceKind::Knight) => "n",
        (Color::Black, PieceKind::Pawn) => "p",
    }
}

const fn piece_glyph(piece: Piece) -> Glyph {
    match (piece.color, piece.kind) {
        (Color::White, PieceKind::King) => Glyph::ChessWhiteKing,
        (Color::White, PieceKind::Queen) => Glyph::ChessWhiteQueen,
        (Color::White, PieceKind::Rook) => Glyph::ChessWhiteRook,
        (Color::White, PieceKind::Bishop) => Glyph::ChessWhiteBishop,
        (Color::White, PieceKind::Knight) => Glyph::ChessWhiteKnight,
        (Color::White, PieceKind::Pawn) => Glyph::ChessWhitePawn,
        (Color::Black, PieceKind::King) => Glyph::ChessBlackKing,
        (Color::Black, PieceKind::Queen) => Glyph::ChessBlackQueen,
        (Color::Black, PieceKind::Rook) => Glyph::ChessBlackRook,
        (Color::Black, PieceKind::Bishop) => Glyph::ChessBlackBishop,
        (Color::Black, PieceKind::Knight) => Glyph::ChessBlackKnight,
        (Color::Black, PieceKind::Pawn) => Glyph::ChessBlackPawn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{AppRunner, Chrome, Command, DisplayMetrics, StoreRequest};

    fn runner(bytes: Option<Vec<u8>>) -> (AppRunner<ChessBoardApp>, Vec<Command>) {
        let mut runner = AppRunner::with_metrics(
            ChessBoardApp::default(),
            DisplayMetrics {
                width: 1264,
                height: 1680,
                pixels_per_inch: 300,
                ..DisplayMetrics::default()
            },
        );
        assert!(runner.start().iter().any(|command| matches!(
            command, Command::Store(StoreRequest::Load { key }) if key == PUZZLES_FILE
        )));
        let commands = runner.store_result(StoreResult::Loaded {
            key: PUZZLES_FILE.into(),
            value: bytes,
        });
        (runner, commands)
    }

    #[test]
    fn flipped_display_reverses_board_and_algebraic_notation() {
        assert_eq!(board_square(0, false), 0);
        assert_eq!(board_square(CELLS - 1, false), CELLS - 1);
        assert_eq!(board_square(0, true), CELLS - 1);
        assert_eq!(board_square(CELLS - 1, true), 0);
        assert_eq!(square_label(0, None), "a8");
        assert_eq!(square_label(CELLS - 1, None), "h1");
    }

    #[test]
    fn puzzle_navigation_sets_orientation_and_keeps_board_controls() {
        let (mut runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));
        assert_eq!(runner.app().puzzles.len(), 10);
        assert!(!runner.app().flipped);
        let initial = runner.app().board.clone();
        let occupied = (0..CELLS)
            .find(|&square| initial.piece_at(square).is_some())
            .unwrap();
        let empty = (0..CELLS)
            .find(|&square| initial.piece_at(square).is_none())
            .unwrap();
        runner.action(action_id(&square_action(occupied)));
        runner.action(action_id(&square_action(empty)));
        assert_ne!(runner.app().board, initial);
        runner.action(action_id(FLIP));
        assert!(runner.app().flipped);
        runner.action(action_id(RESET));
        assert_eq!(runner.app().board, initial);
        assert!(runner.app().flipped, "reset should preserve a manual flip");
        runner.page_turn(true);
        assert_eq!(runner.app().puzzle_index, 1);
        assert!(runner.app().flipped, "black puzzle should face black");
        runner.action(action_id(FLIP));
        assert!(!runner.app().flipped);
        runner.page_turn(false);
        assert_eq!(runner.app().puzzle_index, 0);
        assert!(!runner.app().flipped, "white puzzle should face white");
        runner.page_turn(false);
        assert_eq!(runner.app().puzzle_index, 0);
        for _ in 0..20 {
            runner.page_turn(true);
        }
        assert_eq!(runner.app().puzzle_index, 9);
        assert!(runner
            .action(action_id(EXIT))
            .iter()
            .any(|command| matches!(command, Command::Exit)));
    }

    #[test]
    fn missing_file_creates_examples_but_invalid_file_is_preserved() {
        let (_, commands) = runner(None);
        assert!(commands.iter().any(|command| matches!(
            command, Command::Store(StoreRequest::Save { key, value })
                if key == PUZZLES_FILE && value.as_slice() == EXAMPLE_PUZZLES
        )));
        let (runner, commands) = runner(Some(b"invalid JSON".to_vec()));
        assert!(runner.app().file_error.is_some());
        assert_eq!(runner.app().puzzles.len(), 10);
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::Store(StoreRequest::Save { .. }))));
    }

    #[test]
    fn example_descriptions_and_toolbar_fit_the_kobo_panel() {
        let (mut runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));
        for index in 0..runner.app().puzzles.len() {
            runner.app_mut().select_puzzle(index);
            let screen = runner.app().screen();
            let diagnostics =
                screen.diagnostics(&runner.context().metrics(), &Chrome::measuring(false));
            assert!(
                !diagnostics.has_errors(),
                "puzzle {index}: {:?}",
                diagnostics.issues
            );
            let rect_for = |action| {
                diagnostics
                    .layout
                    .nodes
                    .iter()
                    .find(|node| node.kind.acts_on() == Some(action))
                    .expect("action has a touch target")
                    .rect
            };
            let board_frame = diagnostics
                .layout
                .nodes
                .iter()
                .find(|node| matches!(node.kind, kobo_sdk::LayoutKind::ChessFrame { .. }))
                .expect("board has an outer frame")
                .rect;
            let flip = rect_for(action_id(FLIP));
            assert!(
                (flip.x + flip.width - board_frame.x - board_frame.width).abs() <= 1,
                "toolbar should align with the outer board border"
            );
            for action in [RESET, FLIP] {
                let button = rect_for(action_id(action));
                assert!(
                    (button.width - button.height).abs() <= 1,
                    "button should be square"
                );
            }
        }
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("eink-chess", ChessBoardApp::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("eink-chess: {error}");
            ExitCode::FAILURE
        }
    }
}
