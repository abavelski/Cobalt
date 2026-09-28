mod board;
mod progress;
mod puzzle;

use board::{Board, Color, Piece, PieceKind, TapResult};
use kobo_sdk::{
    action_id, ActionId, BandAlign, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder,
    SlotWidth, StoreResult,
};
use progress::Progress;
use puzzle::{parse_puzzle_file, Puzzle, PuzzleCollection};
use std::{collections::VecDeque, process::ExitCode};

const SIDE: usize = 8;
const CELLS: usize = SIDE * SIDE;
const RESET: &str = "reset";
const FLIP: &str = "flip";
const EXIT: &str = "exit";
const MODE_TOGGLE: &str = "mode-toggle";
const PUZZLE_PICKER: &str = "puzzle-picker";
const PUZZLE_PICKER_CANCEL: &str = "puzzle-picker-cancel";
const COLLECTION_ACTION_PREFIX: &str = "collection-";
const PROMOTE_QUEEN: &str = "promote-queen";
const PROMOTE_ROOK: &str = "promote-rook";
const PROMOTE_BISHOP: &str = "promote-bishop";
const PROMOTE_KNIGHT: &str = "promote-knight";
const PROMOTION_CANCEL: &str = "promotion-cancel";
const PUZZLES_FILE: &str = "puzzles.json";
const PROGRESS_KEY: &str = "progress.v1";
const EXAMPLE_PUZZLES: &[u8] = include_bytes!("puzzles.json");

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum BoardMode {
    #[default]
    Solution,
    FreeBoard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SolutionFeedback {
    Correct,
    Wrong,
    Complete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingPromotion {
    before: Board,
    from: usize,
    to: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UciMove {
    from: usize,
    to: usize,
    promotion: Option<PieceKind>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CollectionEntry {
    key: String,
    title: Option<String>,
    valid: bool,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CollectionLoadKind {
    Metadata,
    Initial,
    Selection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CollectionLoad {
    key: String,
    kind: CollectionLoadKind,
}

#[derive(Default)]
struct ChessBoardApp {
    board: Board,
    puzzles: Vec<Puzzle>,
    puzzle_index: usize,
    solution_ply: usize,
    solution_feedback: Option<SolutionFeedback>,
    mode: BoardMode,
    pending_promotion: Option<PendingPromotion>,
    collections: Vec<CollectionEntry>,
    active_collection: Option<String>,
    collection_load: Option<CollectionLoad>,
    metadata_queue: VecDeque<String>,
    picker_open: bool,
    creating_default: bool,
    progress: Progress,
    progress_ready: bool,
    progress_durable: bool,
    progress_dirty: bool,
    progress_generation: u64,
    progress_saving: Option<u64>,
    progress_warning: Option<String>,
    file_error: Option<String>,
    sleeping: bool,
    flipped: bool,
}

impl ChessBoardApp {
    fn activate_collection(&mut self, key: Option<String>, collection: PuzzleCollection) {
        self.puzzles = collection.puzzles;
        self.active_collection = key;
        self.file_error = None;
        let index = self
            .active_collection
            .as_deref()
            .and_then(|key| self.progress.file(key))
            .and_then(|file| file.current_puzzle_id.as_deref())
            .and_then(|id| self.puzzles.iter().position(|puzzle| puzzle.id == id))
            .unwrap_or(0);
        self.select_puzzle(index);
    }

    fn show_examples_fallback(&mut self, error: impl Into<String>) {
        let collection =
            parse_puzzle_file(EXAMPLE_PUZZLES).expect("bundled example puzzles are valid");
        self.activate_collection(None, collection);
        self.file_error = Some(error.into());
    }

    fn begin_collection_discovery(&mut self, context: &mut Context, keys: Vec<String>) {
        let mut keys = keys
            .into_iter()
            .filter(|key| is_puzzle_collection_key(key))
            .collect::<Vec<_>>();
        keys.sort();
        keys.dedup();

        self.collections = keys
            .iter()
            .map(|key| CollectionEntry {
                key: key.clone(),
                title: None,
                valid: false,
                error: None,
            })
            .collect();
        self.metadata_queue = keys.into();
        self.collection_load = None;

        if self.collections.is_empty() {
            self.creating_default = true;
            context.store().save(PUZZLES_FILE, EXAMPLE_PUZZLES.to_vec());
        } else {
            self.load_next_metadata(context);
        }
    }

    fn load_next_metadata(&mut self, context: &mut Context) {
        if let Some(key) = self.metadata_queue.pop_front() {
            self.collection_load = Some(CollectionLoad {
                key: key.clone(),
                kind: CollectionLoadKind::Metadata,
            });
            context.store().load(key);
        } else {
            self.load_initial_collection(context);
        }
    }

    fn load_initial_collection(&mut self, context: &mut Context) {
        let remembered = self.progress.active_file.as_deref().and_then(|key| {
            self.collections
                .iter()
                .find(|entry| entry.key == key && entry.valid)
        });
        let preferred = remembered
            .or_else(|| {
                self.collections
                    .iter()
                    .find(|entry| entry.key == PUZZLES_FILE && entry.valid)
            })
            .or_else(|| self.collections.iter().find(|entry| entry.valid))
            .map(|entry| entry.key.clone());

        let Some(key) = preferred else {
            self.collection_load = None;
            self.show_examples_fallback(
                "No valid puzzle collection could be loaded; showing bundled examples.",
            );
            self.show(context);
            return;
        };

        self.collection_load = Some(CollectionLoad {
            key: key.clone(),
            kind: CollectionLoadKind::Initial,
        });
        context.store().load(key);
    }

    fn request_collection(&mut self, context: &mut Context, key: String) {
        self.picker_open = false;
        self.remember_current_puzzle();
        if self.active_collection.as_deref() == Some(key.as_str()) {
            self.show(context);
            return;
        }
        if self.collection_load.is_some() {
            return;
        }

        self.file_error = None;
        self.collection_load = Some(CollectionLoad {
            key: key.clone(),
            kind: CollectionLoadKind::Selection,
        });
        context.store().load(key);
        self.show(context);
    }

    fn update_collection_metadata(&mut self, key: &str, parsed: &Result<PuzzleCollection, String>) {
        let Some(entry) = self.collections.iter_mut().find(|entry| entry.key == key) else {
            return;
        };
        match parsed {
            Ok(collection) => {
                entry.title = collection.title.clone();
                entry.valid = true;
                entry.error = None;
            }
            Err(error) => {
                entry.title = None;
                entry.valid = false;
                entry.error = Some(error.clone());
            }
        }
    }

    fn select_puzzle(&mut self, index: usize) {
        let puzzle = &self.puzzles[index];
        self.board = Board::from_fen(&puzzle.fen).expect("validated puzzle FEN");
        self.flipped = puzzle.side_to_move() == Color::Black;
        self.puzzle_index = index;
        self.reset_attempt();
    }

    fn reset_attempt(&mut self) {
        self.solution_ply = 0;
        self.solution_feedback = None;
        self.pending_promotion = None;
    }

    fn note_progress_change(&mut self) {
        if self.progress_ready && self.progress_durable {
            self.progress_generation = self.progress_generation.wrapping_add(1);
            self.progress_dirty = true;
        }
    }

    fn remember_current_puzzle(&mut self) {
        let (Some(key), Some(puzzle)) = (
            self.active_collection.clone(),
            self.puzzles.get(self.puzzle_index),
        ) else {
            return;
        };
        let puzzle_id = puzzle.id.clone();
        if self.progress.remember_puzzle(&key, &puzzle_id) {
            self.note_progress_change();
        }
    }

    fn remember_active_collection(&mut self) {
        let Some(key) = self.active_collection.clone() else {
            return;
        };
        let changed_active = self.progress.set_active_file(&key);
        let puzzle_id = self
            .puzzles
            .get(self.puzzle_index)
            .map(|puzzle| puzzle.id.clone());
        let changed_puzzle = puzzle_id
            .as_deref()
            .is_some_and(|id| self.progress.remember_puzzle(&key, id));
        if changed_active || changed_puzzle {
            self.note_progress_change();
        }
    }

    fn mark_current_solved(&mut self) {
        let (Some(key), Some(puzzle)) = (
            self.active_collection.clone(),
            self.puzzles.get(self.puzzle_index),
        ) else {
            return;
        };
        let puzzle_id = puzzle.id.clone();
        if self.progress.mark_solved(&key, &puzzle_id) {
            self.note_progress_change();
        }
    }

    fn current_is_solved(&self) -> bool {
        let (Some(key), Some(puzzle)) = (
            self.active_collection.as_deref(),
            self.puzzles.get(self.puzzle_index),
        ) else {
            return false;
        };
        self.progress.is_solved(key, &puzzle.id)
    }

    fn save_progress(&mut self, context: &mut Context) {
        if !self.progress_ready
            || !self.progress_durable
            || !self.progress_dirty
            || self.progress_saving.is_some()
        {
            return;
        }
        match self.progress.to_bytes() {
            Ok(bytes) => {
                let generation = self.progress_generation;
                self.progress_saving = Some(generation);
                context.store().save(PROGRESS_KEY, bytes);
            }
            Err(_) => {
                self.progress_warning = Some("Progress not saved".into());
            }
        }
    }

    fn handle_progress_load(&mut self, result: StoreResult) {
        self.progress_ready = true;
        match result {
            StoreResult::Loaded {
                value: Some(bytes), ..
            } => match Progress::parse(&bytes) {
                Ok(progress) => {
                    self.progress = progress;
                    self.progress_durable = true;
                    self.progress_warning = None;
                }
                Err(error) => {
                    self.progress = Progress::new();
                    self.progress_durable = false;
                    self.progress_warning = Some(format!("Progress file needs fixing: {error}"));
                }
            },
            StoreResult::Loaded { value: None, .. } => {
                self.progress = Progress::new();
                self.progress_durable = true;
                self.progress_warning = None;
            }
            StoreResult::Denied(error) => {
                self.progress = Progress::new();
                self.progress_durable = false;
                self.progress_warning = Some(format!(
                    "Progress could not be read ({error}); progress will not be saved."
                ));
            }
            _ => {
                self.progress = Progress::new();
                self.progress_durable = false;
                self.progress_warning =
                    Some("Progress could not be read; progress will not be saved.".into());
            }
        }
    }

    fn handle_progress_save(&mut self, context: &mut Context, result: StoreResult) {
        let Some(saved_generation) = self.progress_saving.take() else {
            return;
        };
        match result {
            StoreResult::Saved { .. } => {
                if saved_generation == self.progress_generation {
                    self.progress_dirty = false;
                    self.progress_warning = None;
                } else {
                    self.progress_dirty = true;
                    self.save_progress(context);
                }
            }
            StoreResult::Denied(_)
            | StoreResult::Loaded { .. }
            | StoreResult::Forgotten { .. }
            | StoreResult::Keys(_)
            | StoreResult::ShelfWritten { .. }
            | StoreResult::ShelfRead { .. }
            | StoreResult::ShelfRemoved { .. }
            | StoreResult::Shelf(_) => {
                self.progress_dirty = true;
                self.progress_warning = Some("Progress not saved".into());
            }
        }
    }

    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }

    fn screen(&self) -> Screen {
        let selected = self.board.selected();
        let feedback_glyph = if self.mode == BoardMode::Solution {
            match self.solution_feedback {
                Some(SolutionFeedback::Complete) => Some(Glyph::ThumbUp),
                Some(SolutionFeedback::Wrong) => Some(Glyph::ThumbDown),
                _ => None,
            }
        } else {
            None
        };

        let cells = (0..CELLS).map(|display_square| {
            let square = board_square(display_square, self.flipped);
            let piece = self.board.piece_at(square);
            (
                square_action(square),
                square_label(square, piece),
                if display_square == 27 {
                    feedback_glyph.or_else(|| piece.map(piece_glyph))
                } else {
                    piece.map(piece_glyph)
                },
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
        let free_board = self.mode == BoardMode::FreeBoard;
        let mut screen = ScreenBuilder::new("chessboard")
            .top_bar(title)
            .top_bar_glyph(EXIT, "Return to Kobo reader", Glyph::Close)
            .top_bar_action(PUZZLE_PICKER, "Puzzles")
            .board_with_selection(SIDE as u8, cells)
            .band(
                BandAlign::Middle,
                [
                    (
                        SlotWidth::Fill,
                        Box::new(|slot: ScreenBuilder| slot)
                            as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>,
                    ),
                    (
                        // Three 10 mm targets with two 1 mm gaps.
                        SlotWidth::Fixed(320),
                        Box::new(move |slot| {
                            slot.controls_with_selection(
                                3,
                                [
                                    (MODE_TOGGLE, "Toggle free board", Glyph::Grid, free_board),
                                    (RESET, "Reset position", Glyph::Refresh, false),
                                    (FLIP, "Flip board", Glyph::SwapVertical, false),
                                ],
                            )
                        }),
                    ),
                ],
            );

        if let Some(puzzle) = self.puzzles.get(self.puzzle_index) {
            let status = match self.mode {
                BoardMode::Solution => match puzzle.side_to_move() {
                    Color::White => "White to move",
                    Color::Black => "Black to move",
                },
                BoardMode::FreeBoard => "Free board — moves are not graded",
            };
            let status = if self.mode == BoardMode::Solution
                && self.solution_feedback == Some(SolutionFeedback::Correct)
            {
                format!("{status} · Correct")
            } else if self.current_is_solved() {
                format!("{status} · Solved")
            } else {
                status.to_owned()
            };
            screen = screen.secondary(status);
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
        if let Some(warning) = self.progress_warning.as_ref() {
            screen = screen.banner(BannerLevel::Attention, warning.clone());
        }

        if self.pending_promotion.is_some() {
            screen = screen.modal("Promote pawn", |modal| {
                modal
                    .choose(
                        "Choose promotion piece",
                        [
                            (PROMOTE_QUEEN, "Queen"),
                            (PROMOTE_ROOK, "Rook"),
                            (PROMOTE_BISHOP, "Bishop"),
                            (PROMOTE_KNIGHT, "Knight"),
                        ],
                    )
                    .button(PROMOTION_CANCEL, "Cancel")
            });
        }

        if self.picker_open {
            screen = screen.modal("Puzzle collections", |modal| {
                modal
                    .rows(self.collections.iter().enumerate().map(|(index, entry)| {
                        (
                            collection_action(index),
                            collection_display_name(entry),
                            collection_summary(entry, self.active_collection.as_deref()),
                            Glyph::Grid,
                        )
                    }))
                    .button(PUZZLE_PICKER_CANCEL, "Cancel")
            });
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
        self.remember_current_puzzle();
        self.save_progress(context);
        self.show(context);
    }

    fn handle_square_tap(&mut self, square: usize) -> bool {
        if (self.mode == BoardMode::Solution
            && self.solution_feedback == Some(SolutionFeedback::Complete))
            || self.pending_promotion.is_some()
        {
            return false;
        }

        let before = self.board.clone();
        match self.board.tap(square) {
            TapResult::NoChange => false,
            TapResult::SelectionChanged => true,
            TapResult::Moved { from, to } => {
                if self.mode == BoardMode::Solution {
                    let attempted = uci_move(from, to, None);
                    self.check_solver_move(before, &attempted);
                }
                true
            }
            TapResult::Promotion { from, to, .. } => {
                self.pending_promotion = Some(PendingPromotion { before, from, to });
                true
            }
        }
    }

    fn finish_promotion(&mut self, kind: PieceKind) {
        let Some(pending) = self.pending_promotion.take() else {
            return;
        };
        let before = pending.before;
        self.board = before.clone();
        self.board.clear_selection();
        if !self.board.promote_pawn(pending.from, pending.to, kind) {
            self.board = before;
            self.board.clear_selection();
            self.file_error = Some("The staged pawn promotion could not be applied.".into());
            return;
        }

        if self.mode == BoardMode::Solution {
            let attempted = uci_move(pending.from, pending.to, Some(kind));
            self.check_solver_move(before, &attempted);
        }
    }

    fn set_mode(&mut self, mode: BoardMode) {
        if self.mode == mode {
            return;
        }

        self.pending_promotion = None;
        self.board.clear_selection();
        match mode {
            BoardMode::FreeBoard => {
                self.mode = BoardMode::FreeBoard;
                self.solution_feedback = None;
            }
            BoardMode::Solution => {
                self.board.reset();
                self.reset_attempt();
                self.mode = BoardMode::Solution;
            }
        }
    }

    fn cancel_promotion(&mut self) {
        let Some(pending) = self.pending_promotion.take() else {
            return;
        };
        self.board = pending.before;
        self.board.clear_selection();
    }

    fn check_solver_move(&mut self, before: Board, attempted: &str) {
        let original_ply = self.solution_ply;
        let expected = self
            .puzzles
            .get(self.puzzle_index)
            .and_then(|puzzle| puzzle.solution.get(original_ply))
            .cloned();

        let Some(expected) = expected else {
            self.restore_wrong_move(before);
            return;
        };

        if attempted != expected {
            self.restore_wrong_move(before);
            return;
        }

        self.solution_ply += 1;

        let reply = self
            .puzzles
            .get(self.puzzle_index)
            .and_then(|puzzle| puzzle.solution.get(self.solution_ply))
            .cloned();

        if let Some(reply) = reply {
            if !self.apply_stored_move(&reply) {
                self.restore_invalid_reply(before, original_ply, &reply);
                return;
            }
            self.solution_ply += 1;
        }

        let solution_len = self
            .puzzles
            .get(self.puzzle_index)
            .map_or(0, |puzzle| puzzle.solution.len());
        let complete = self.solution_ply >= solution_len;
        self.solution_feedback = Some(if complete {
            SolutionFeedback::Complete
        } else {
            SolutionFeedback::Correct
        });
        if complete {
            self.mark_current_solved();
        }
    }

    fn apply_stored_move(&mut self, movement: &str) -> bool {
        let Some(movement) = parse_uci_move(movement) else {
            return false;
        };
        match movement.promotion {
            Some(kind) => self.board.promote_pawn(movement.from, movement.to, kind),
            None => self.board.move_piece(movement.from, movement.to),
        }
    }

    fn restore_wrong_move(&mut self, before: Board) {
        self.board = before;
        self.board.clear_selection();
        self.solution_feedback = Some(SolutionFeedback::Wrong);
    }

    fn restore_invalid_reply(&mut self, before: Board, original_ply: usize, reply: &str) {
        self.board = before;
        self.board.clear_selection();
        self.solution_ply = original_ply;
        self.solution_feedback = None;
        let puzzle_id = self
            .puzzles
            .get(self.puzzle_index)
            .map_or("unknown", |puzzle| puzzle.id.as_str());
        self.file_error = Some(format!(
            "Puzzle {puzzle_id}: stored reply {reply} could not be applied."
        ));
    }
}

impl KoboApp for ChessBoardApp {
    fn on_start(&mut self, context: &mut Context) {
        context.store().list();
        context.store().load(PROGRESS_KEY);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        match result {
            StoreResult::Keys(keys) => self.begin_collection_discovery(context, keys),
            StoreResult::Denied(error) => {
                self.show_examples_fallback(format!(
                    "Puzzle collections could not be listed ({error}); showing bundled examples."
                ));
                self.show(context);
            }
            _ => {}
        }
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key == PROGRESS_KEY {
            self.handle_progress_save(context, result);
            self.show(context);
            return;
        }
        if key != PUZZLES_FILE || !self.creating_default {
            return;
        }
        self.creating_default = false;
        match result {
            StoreResult::Saved { .. } => context.store().list(),
            StoreResult::Denied(error) => {
                self.show_examples_fallback(format!(
                    "The default puzzle collection could not be created ({error}); showing bundled examples."
                ));
                self.show(context);
            }
            _ => {
                self.show_examples_fallback(
                    "The default puzzle collection could not be created; showing bundled examples.",
                );
                self.show(context);
            }
        }
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key == PROGRESS_KEY {
            self.handle_progress_load(result);
            return;
        }

        let Some(load) = self.collection_load.take() else {
            return;
        };
        if load.key != key {
            self.show_examples_fallback(format!(
                "Unexpected puzzle collection response for {key}; showing bundled examples."
            ));
            self.show(context);
            return;
        }

        let parsed = match result {
            StoreResult::Loaded {
                value: Some(bytes), ..
            } => parse_puzzle_file(&bytes).map_err(|error| format!("{key}: {error}")),
            StoreResult::Loaded { value: None, .. } => {
                Err(format!("{key}: the puzzle collection no longer exists."))
            }
            StoreResult::Denied(error) => Err(format!("{key}: could not be read ({error}).")),
            _ => Err(format!("{key}: unexpected store response.")),
        };

        match load.kind {
            CollectionLoadKind::Metadata => {
                self.update_collection_metadata(key, &parsed);
                self.load_next_metadata(context);
            }
            CollectionLoadKind::Initial => match parsed {
                Ok(collection) => {
                    let title = collection.title.clone();
                    if let Some(entry) = self.collections.iter_mut().find(|entry| entry.key == key)
                    {
                        entry.title = title;
                        entry.valid = true;
                        entry.error = None;
                    }
                    self.activate_collection(Some(key.to_owned()), collection);
                    self.show(context);
                }
                Err(error) => {
                    if let Some(entry) = self.collections.iter_mut().find(|entry| entry.key == key)
                    {
                        entry.valid = false;
                        entry.error = Some(error.clone());
                    }
                    self.show_examples_fallback(format!(
                        "{error} Showing bundled examples instead."
                    ));
                    self.show(context);
                }
            },
            CollectionLoadKind::Selection => match parsed {
                Ok(collection) => {
                    let title = collection.title.clone();
                    if let Some(entry) = self.collections.iter_mut().find(|entry| entry.key == key)
                    {
                        entry.title = title;
                        entry.valid = true;
                        entry.error = None;
                    }
                    self.activate_collection(Some(key.to_owned()), collection);
                    self.remember_active_collection();
                    self.save_progress(context);
                    self.show(context);
                }
                Err(error) => {
                    if let Some(entry) = self.collections.iter_mut().find(|entry| entry.key == key)
                    {
                        entry.valid = false;
                        entry.error = Some(error.clone());
                    }
                    self.file_error = Some(error);
                    self.show(context);
                }
            },
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.pending_promotion.is_some() {
            if action == action_id(PROMOTION_CANCEL) {
                self.cancel_promotion();
                self.show(context);
            } else if let Some(kind) = promotion_kind_for_action(action) {
                self.finish_promotion(kind);
                self.save_progress(context);
                self.show(context);
            }
            return;
        }

        if self.picker_open {
            if action == action_id(PUZZLE_PICKER_CANCEL) {
                self.picker_open = false;
                self.show(context);
                return;
            }
            for index in 0..self.collections.len() {
                if action == action_id(&collection_action(index)) {
                    let key = self.collections[index].key.clone();
                    self.request_collection(context, key);
                    return;
                }
            }
            return;
        }

        if action == action_id(PUZZLE_PICKER) {
            if self.collection_load.is_none() && !self.collections.is_empty() {
                self.picker_open = true;
                self.show(context);
            }
            return;
        }

        if action == action_id(EXIT) {
            context.exit();
            return;
        }

        if action == action_id(MODE_TOGGLE) {
            self.set_mode(match self.mode {
                BoardMode::Solution => BoardMode::FreeBoard,
                BoardMode::FreeBoard => BoardMode::Solution,
            });
            self.show(context);
            return;
        }

        if action == action_id(RESET) {
            self.board.reset();
            self.reset_attempt();
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
                if self.handle_square_tap(square) {
                    self.save_progress(context);
                    self.show(context);
                }
                return;
            }
        }
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.pending_promotion.is_none() {
            self.turn_puzzle(context, forward);
        }
    }

    fn on_suspend(&mut self, context: &mut Context) {
        self.save_progress(context);
        self.sleeping = true;
        self.show(context);
    }

    fn on_background(&mut self, context: &mut Context) {
        self.save_progress(context);
    }

    fn on_exit(&mut self, context: &mut Context) {
        self.save_progress(context);
    }

    fn on_resume(&mut self, context: &mut Context) {
        self.sleeping = false;
        self.show(context);
    }
}

fn is_puzzle_collection_key(key: &str) -> bool {
    key == PUZZLES_FILE
        || key
            .strip_prefix("puzzles-")
            .and_then(|name| name.strip_suffix(".json"))
            .is_some_and(|name| !name.is_empty())
}

fn collection_action(index: usize) -> String {
    format!("{COLLECTION_ACTION_PREFIX}{index}")
}

fn collection_display_name(entry: &CollectionEntry) -> String {
    entry
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(&entry.key)
        .to_owned()
}

fn collection_summary(entry: &CollectionEntry, active: Option<&str>) -> String {
    let current = active == Some(entry.key.as_str());
    if current && entry.title.is_some() {
        format!("{} · current", entry.key)
    } else if current {
        "Current collection".into()
    } else if !entry.valid {
        format!("{} · could not be read", entry.key)
    } else if entry.title.is_some() {
        entry.key.clone()
    } else {
        String::new()
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

fn square_name(square: usize) -> String {
    debug_assert!(square < CELLS);
    let file = (b'a' + (square % SIDE) as u8) as char;
    let rank = (b'8' - (square / SIDE) as u8) as char;
    format!("{file}{rank}")
}

fn square_from_name(name: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.len() != 2 || !(b'a'..=b'h').contains(&bytes[0]) || !(b'1'..=b'8').contains(&bytes[1])
    {
        return None;
    }
    let file = usize::from(bytes[0] - b'a');
    let rank_from_top = usize::from(b'8' - bytes[1]);
    Some(rank_from_top * SIDE + file)
}

fn uci_move(from: usize, to: usize, promotion: Option<PieceKind>) -> String {
    let mut movement = format!("{}{}", square_name(from), square_name(to));
    if let Some(kind) = promotion {
        movement.push(promotion_suffix(kind).expect("only promotable piece kinds reach UCI"));
    }
    movement
}

fn parse_uci_move(movement: &str) -> Option<UciMove> {
    let bytes = movement.as_bytes();
    if bytes.len() != 4 && bytes.len() != 5 {
        return None;
    }
    let from = std::str::from_utf8(&bytes[..2])
        .ok()
        .and_then(square_from_name)?;
    let to = std::str::from_utf8(&bytes[2..4])
        .ok()
        .and_then(square_from_name)?;
    let promotion = if bytes.len() == 5 {
        Some(promotion_kind(bytes[4])?)
    } else {
        None
    };
    Some(UciMove {
        from,
        to,
        promotion,
    })
}

const fn promotion_kind(suffix: u8) -> Option<PieceKind> {
    match suffix {
        b'q' => Some(PieceKind::Queen),
        b'r' => Some(PieceKind::Rook),
        b'b' => Some(PieceKind::Bishop),
        b'n' => Some(PieceKind::Knight),
        _ => None,
    }
}

const fn promotion_suffix(kind: PieceKind) -> Option<char> {
    match kind {
        PieceKind::Queen => Some('q'),
        PieceKind::Rook => Some('r'),
        PieceKind::Bishop => Some('b'),
        PieceKind::Knight => Some('n'),
        PieceKind::Pawn | PieceKind::King => None,
    }
}

fn promotion_kind_for_action(action: ActionId) -> Option<PieceKind> {
    if action == action_id(PROMOTE_QUEEN) {
        Some(PieceKind::Queen)
    } else if action == action_id(PROMOTE_ROOK) {
        Some(PieceKind::Rook)
    } else if action == action_id(PROMOTE_BISHOP) {
        Some(PieceKind::Bishop)
    } else if action == action_id(PROMOTE_KNIGHT) {
        Some(PieceKind::Knight)
    } else {
        None
    }
}

fn square_label(square: usize, piece: Option<Piece>) -> String {
    let name = square_name(square);
    match piece {
        Some(piece) => format!("{name} {}", piece_label(piece)),
        None => name,
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

    const PROMOTION_PUZZLES: &[u8] = include_bytes!("promotion-puzzles.json");
    const ALT_PUZZLES: &[u8] = br#"{
      "version": 1,
      "title": "Endgames",
      "puzzles": [
        {
          "id": "end-1",
          "fen": "7k/8/5KQ1/8/8/8/8/8 w - - 0 1",
          "description": "Endgame one.",
          "solution": ["g6g7"]
        },
        {
          "id": "end-2",
          "fen": "8/8/8/8/8/5kq1/8/7K b - - 0 1",
          "description": "Endgame two.",
          "solution": ["g3g2"]
        }
      ]
    }"#;

    fn new_runner() -> AppRunner<ChessBoardApp> {
        AppRunner::with_metrics(
            ChessBoardApp::default(),
            DisplayMetrics {
                width: 1264,
                height: 1680,
                pixels_per_inch: 300,
                ..DisplayMetrics::default()
            },
        )
    }

    fn runner(bytes: Option<Vec<u8>>) -> (AppRunner<ChessBoardApp>, Vec<Command>) {
        match bytes {
            Some(bytes) => runner_with_collections(vec![(PUZZLES_FILE, bytes)]),
            None => {
                let mut runner = new_runner();
                let mut commands = runner.start();
                assert!(commands
                    .iter()
                    .any(|command| matches!(command, Command::Store(StoreRequest::List))));
                assert!(commands.iter().any(|command| matches!(
                    command,
                    Command::Store(StoreRequest::Load { key }) if key == PROGRESS_KEY
                )));

                let next = runner.store_result(StoreResult::Keys(Vec::new()));
                assert!(next.iter().any(|command| matches!(
                    command,
                    Command::Store(StoreRequest::Save { key, value })
                        if key == PUZZLES_FILE && value.as_slice() == EXAMPLE_PUZZLES
                )));
                commands.extend(next);

                let next = runner.store_result(StoreResult::Loaded {
                    key: PROGRESS_KEY.into(),
                    value: None,
                });
                commands.extend(next);

                let next = runner.store_result(StoreResult::Saved {
                    key: PUZZLES_FILE.into(),
                });
                assert!(next
                    .iter()
                    .any(|command| matches!(command, Command::Store(StoreRequest::List))));
                commands.extend(next);

                let next = runner.store_result(StoreResult::Keys(vec![PUZZLES_FILE.to_owned()]));
                commands.extend(next);

                let next = runner.store_result(StoreResult::Loaded {
                    key: PUZZLES_FILE.into(),
                    value: Some(EXAMPLE_PUZZLES.to_vec()),
                });
                commands.extend(next);

                let next = runner.store_result(StoreResult::Loaded {
                    key: PUZZLES_FILE.into(),
                    value: Some(EXAMPLE_PUZZLES.to_vec()),
                });
                commands.extend(next);

                (runner, commands)
            }
        }
    }

    fn runner_with_collections(
        files: Vec<(&str, Vec<u8>)>,
    ) -> (AppRunner<ChessBoardApp>, Vec<Command>) {
        runner_with_state(files, None)
    }

    fn runner_with_state(
        files: Vec<(&str, Vec<u8>)>,
        progress: Option<Vec<u8>>,
    ) -> (AppRunner<ChessBoardApp>, Vec<Command>) {
        let mut runner = new_runner();
        let mut commands = runner.start();
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Store(StoreRequest::List))));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(StoreRequest::Load { key }) if key == PROGRESS_KEY
        )));

        let keys = files.iter().map(|(key, _)| (*key).to_owned()).collect();
        let next = runner.store_result(StoreResult::Keys(keys));
        commands.extend(next);

        let next = runner.store_result(StoreResult::Loaded {
            key: PROGRESS_KEY.into(),
            value: progress,
        });
        commands.extend(next);

        let mut metadata_keys = files
            .iter()
            .map(|(key, _)| (*key).to_owned())
            .filter(|key| is_puzzle_collection_key(key))
            .collect::<Vec<_>>();
        metadata_keys.sort();
        metadata_keys.dedup();

        for key in metadata_keys {
            let bytes = files
                .iter()
                .find(|(candidate, _)| *candidate == key)
                .map(|(_, bytes)| bytes.clone())
                .expect("metadata key has bytes");
            let next = runner.store_result(StoreResult::Loaded {
                key,
                value: Some(bytes),
            });
            commands.extend(next);
        }

        if let Some(CollectionLoad {
            key,
            kind: CollectionLoadKind::Initial,
        }) = runner.app().collection_load.clone()
        {
            let bytes = files
                .iter()
                .find(|(candidate, _)| *candidate == key)
                .map(|(_, bytes)| bytes.clone())
                .expect("initial collection has bytes");
            let next = runner.store_result(StoreResult::Loaded {
                key,
                value: Some(bytes),
            });
            commands.extend(next);
        }

        (runner, commands)
    }

    fn progress_bytes(commands: &[Command]) -> Option<Vec<u8>> {
        commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == PROGRESS_KEY => {
                Some(value.clone())
            }
            _ => None,
        })
    }

    fn ack_progress_save(runner: &mut AppRunner<ChessBoardApp>) -> Vec<Command> {
        if runner.app().progress_saving.is_some() {
            runner.store_result(StoreResult::Saved {
                key: PROGRESS_KEY.into(),
            })
        } else {
            Vec::new()
        }
    }

    fn switch_collection(
        runner: &mut AppRunner<ChessBoardApp>,
        key: &str,
        bytes: &[u8],
    ) -> Vec<Command> {
        if runner.app().progress_saving.is_some() {
            ack_progress_save(runner);
        }
        let index = runner
            .app()
            .collections
            .iter()
            .position(|entry| entry.key == key)
            .expect("collection is discovered");
        runner.action(action_id(PUZZLE_PICKER));
        let mut commands = runner.action(action_id(&collection_action(index)));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(StoreRequest::Load { key: requested }) if requested == key
        )));
        commands.extend(runner.store_result(StoreResult::Loaded {
            key: key.to_owned(),
            value: Some(bytes.to_vec()),
        }));
        commands
    }

    fn play(runner: &mut AppRunner<ChessBoardApp>, movement: &str) -> Vec<Command> {
        let movement = parse_uci_move(movement).expect("valid test UCI");
        let mut commands = runner.action(action_id(&square_action(movement.from)));
        commands.extend(runner.action(action_id(&square_action(movement.to))));
        if let Some(kind) = movement.promotion {
            commands.extend(runner.action(action_id(promotion_action(kind))));
        }
        commands
    }

    fn promotion_action(kind: PieceKind) -> &'static str {
        match kind {
            PieceKind::Queen => PROMOTE_QUEEN,
            PieceKind::Rook => PROMOTE_ROOK,
            PieceKind::Bishop => PROMOTE_BISHOP,
            PieceKind::Knight => PROMOTE_KNIGHT,
            PieceKind::Pawn | PieceKind::King => panic!("not a promotion piece"),
        }
    }

    #[test]
    fn flipped_display_reverses_board_but_not_uci_coordinates() {
        assert_eq!(board_square(0, false), 0);
        assert_eq!(board_square(CELLS - 1, false), CELLS - 1);
        assert_eq!(board_square(0, true), CELLS - 1);
        assert_eq!(board_square(CELLS - 1, true), 0);
        assert_eq!(square_label(0, None), "a8");
        assert_eq!(square_label(CELLS - 1, None), "h1");

        let movement = parse_uci_move("e2e4").unwrap();
        assert_eq!(
            uci_move(movement.from, movement.to, movement.promotion),
            "e2e4"
        );
        let flipped_from = (0..CELLS)
            .find(|&display| board_square(display, true) == movement.from)
            .unwrap();
        let flipped_to = (0..CELLS)
            .find(|&display| board_square(display, true) == movement.to)
            .unwrap();
        assert_ne!(flipped_from, movement.from);
        assert_ne!(flipped_to, movement.to);
        assert_eq!(
            uci_move(
                board_square(flipped_from, true),
                board_square(flipped_to, true),
                None
            ),
            "e2e4"
        );
    }

    #[test]
    fn promotion_uci_supports_all_standard_suffixes() {
        let from = square_from_name("a7").unwrap();
        let to = square_from_name("a8").unwrap();
        for (kind, expected) in [
            (PieceKind::Queen, "a7a8q"),
            (PieceKind::Rook, "a7a8r"),
            (PieceKind::Bishop, "a7a8b"),
            (PieceKind::Knight, "a7a8n"),
        ] {
            assert_eq!(uci_move(from, to, Some(kind)), expected);
            assert_eq!(
                parse_uci_move(expected),
                Some(UciMove {
                    from,
                    to,
                    promotion: Some(kind),
                })
            );
        }
    }

    #[test]
    fn wrong_move_is_rejected_without_advancing_solution() {
        let (mut runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));
        let initial = runner.app().board.clone();

        play(&mut runner, "d7d8");

        assert_eq!(runner.app().board, initial);
        assert_eq!(runner.app().solution_ply, 0);
        assert_eq!(
            runner.app().solution_feedback,
            Some(SolutionFeedback::Wrong)
        );
    }

    #[test]
    fn one_move_solution_completes_and_locks_board() {
        let (mut runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));

        play(&mut runner, "d7e8");

        assert_eq!(runner.app().solution_ply, 1);
        assert_eq!(
            runner.app().solution_feedback,
            Some(SolutionFeedback::Complete)
        );
        let solved = runner.app().board.clone();
        let e8 = square_from_name("e8").unwrap();
        runner.action(action_id(&square_action(e8)));
        assert_eq!(runner.app().board, solved);
    }

    #[test]
    fn mate_in_two_auto_plays_reply_then_completes_and_resets() {
        let (mut runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));
        runner.page_turn(true);
        runner.page_turn(true);
        assert_eq!(runner.app().puzzle_index, 2);

        let initial = runner.app().board.clone();
        play(&mut runner, "e2e6");

        assert_eq!(runner.app().solution_ply, 2);
        assert_eq!(
            runner.app().solution_feedback,
            Some(SolutionFeedback::Correct)
        );
        assert!(runner
            .app()
            .board
            .piece_at(square_from_name("f7").unwrap())
            .is_none());
        assert_eq!(
            runner.app().board.piece_at(square_from_name("f8").unwrap()),
            Some(Piece {
                color: Color::Black,
                kind: PieceKind::King,
            })
        );

        play(&mut runner, "e6f7");
        assert_eq!(runner.app().solution_ply, 3);
        assert_eq!(
            runner.app().solution_feedback,
            Some(SolutionFeedback::Complete)
        );

        runner.action(action_id(RESET));
        assert_eq!(runner.app().board, initial);
        assert_eq!(runner.app().solution_ply, 0);
        assert_eq!(runner.app().solution_feedback, None);
    }

    #[test]
    fn cancelling_promotion_leaves_board_and_attempt_unchanged() {
        let (mut runner, _) = runner(Some(PROMOTION_PUZZLES.to_vec()));
        let initial = runner.app().board.clone();

        let movement = parse_uci_move("a7a8q").unwrap();
        runner.action(action_id(&square_action(movement.from)));
        runner.action(action_id(&square_action(movement.to)));
        assert!(runner.app().pending_promotion.is_some());
        assert_eq!(
            runner.app().board.piece_at(movement.from),
            initial.piece_at(movement.from)
        );
        assert_eq!(
            runner.app().board.piece_at(movement.to),
            initial.piece_at(movement.to)
        );

        runner.action(action_id(PROMOTION_CANCEL));

        assert_eq!(runner.app().board, initial);
        assert!(runner.app().pending_promotion.is_none());
        assert_eq!(runner.app().solution_ply, 0);
        assert_eq!(runner.app().solution_feedback, None);
    }

    #[test]
    fn wrong_underpromotion_is_rejected_and_correct_one_completes() {
        let (mut runner, _) = runner(Some(PROMOTION_PUZZLES.to_vec()));
        runner.page_turn(true);
        let initial = runner.app().board.clone();

        let movement = parse_uci_move("b7b8n").unwrap();
        runner.action(action_id(&square_action(movement.from)));
        runner.action(action_id(&square_action(movement.to)));
        runner.action(action_id(PROMOTE_QUEEN));

        assert_eq!(runner.app().board, initial);
        assert_eq!(runner.app().solution_ply, 0);
        assert_eq!(
            runner.app().solution_feedback,
            Some(SolutionFeedback::Wrong)
        );

        play(&mut runner, "b7b8n");
        assert_eq!(
            runner.app().board.piece_at(square_from_name("b8").unwrap()),
            Some(Piece {
                color: Color::White,
                kind: PieceKind::Knight,
            })
        );
        assert_eq!(
            runner.app().solution_feedback,
            Some(SolutionFeedback::Complete)
        );
    }

    #[test]
    fn black_pawn_can_promote_to_an_underpromotion_piece() {
        let (mut runner, _) = runner(Some(PROMOTION_PUZZLES.to_vec()));
        runner.page_turn(true);
        runner.page_turn(true);

        play(&mut runner, "h2h1r");

        assert_eq!(
            runner.app().board.piece_at(square_from_name("h1").unwrap()),
            Some(Piece {
                color: Color::Black,
                kind: PieceKind::Rook,
            })
        );
        assert_eq!(
            runner.app().solution_feedback,
            Some(SolutionFeedback::Complete)
        );
    }

    #[test]
    fn automatic_opponent_promotion_applies_without_opening_modal() {
        let (mut runner, _) = runner(Some(PROMOTION_PUZZLES.to_vec()));
        for _ in 0..3 {
            runner.page_turn(true);
        }

        play(&mut runner, "h2h3");

        assert!(runner.app().pending_promotion.is_none());
        assert_eq!(runner.app().solution_ply, 2);
        assert_eq!(
            runner.app().board.piece_at(square_from_name("a1").unwrap()),
            Some(Piece {
                color: Color::Black,
                kind: PieceKind::Queen,
            })
        );
        assert_eq!(
            runner.app().solution_feedback,
            Some(SolutionFeedback::Correct)
        );
    }

    #[test]
    fn changing_puzzles_clears_transient_solution_feedback_and_promotion() {
        let (mut runner, _) = runner(Some(PROMOTION_PUZZLES.to_vec()));
        let movement = parse_uci_move("a7a8q").unwrap();
        runner.action(action_id(&square_action(movement.from)));
        runner.action(action_id(&square_action(movement.to)));
        assert!(runner.app().pending_promotion.is_some());

        runner.action(action_id(PROMOTION_CANCEL));
        play(&mut runner, "a7a8q");
        assert_eq!(
            runner.app().solution_feedback,
            Some(SolutionFeedback::Complete)
        );

        runner.page_turn(true);

        assert_eq!(runner.app().puzzle_index, 1);
        assert_eq!(runner.app().solution_ply, 0);
        assert_eq!(runner.app().solution_feedback, None);
        assert!(runner.app().pending_promotion.is_none());
    }

    #[test]
    fn free_board_accepts_wrong_moves_without_advancing_solution() {
        let (mut runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));
        let initial = runner.app().board.clone();

        runner.action(action_id(MODE_TOGGLE));
        assert_eq!(runner.app().mode, BoardMode::FreeBoard);
        play(&mut runner, "d7d8");

        assert_ne!(runner.app().board, initial);
        assert_eq!(runner.app().solution_ply, 0);
        assert_eq!(runner.app().solution_feedback, None);
    }

    #[test]
    fn entering_free_board_keeps_the_current_solved_line_position() {
        let (mut runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));
        runner.page_turn(true);
        runner.page_turn(true);
        play(&mut runner, "e2e6");
        assert_eq!(runner.app().solution_ply, 2);
        let current = runner.app().board.clone();

        runner.action(action_id(MODE_TOGGLE));

        assert_eq!(runner.app().mode, BoardMode::FreeBoard);
        assert_eq!(runner.app().board, current);
        assert_eq!(runner.app().solution_ply, 2);
        assert_eq!(runner.app().solution_feedback, None);

        play(&mut runner, "e6e5");
        assert_eq!(runner.app().solution_ply, 2);
    }

    #[test]
    fn returning_to_solution_resets_position_attempt_and_keeps_flip() {
        let (mut runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));
        let initial = runner.app().board.clone();

        runner.action(action_id(MODE_TOGGLE));
        play(&mut runner, "d7d8");
        runner.action(action_id(FLIP));
        assert!(runner.app().flipped);

        runner.action(action_id(MODE_TOGGLE));

        assert_eq!(runner.app().mode, BoardMode::Solution);
        assert_eq!(runner.app().board, initial);
        assert_eq!(runner.app().solution_ply, 0);
        assert_eq!(runner.app().solution_feedback, None);
        assert!(runner.app().flipped);
    }

    #[test]
    fn reset_and_navigation_preserve_free_board_mode() {
        let (mut runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));
        runner.action(action_id(MODE_TOGGLE));
        let initial = runner.app().board.clone();

        play(&mut runner, "d7d8");
        assert_ne!(runner.app().board, initial);

        runner.action(action_id(RESET));
        assert_eq!(runner.app().board, initial);
        assert_eq!(runner.app().mode, BoardMode::FreeBoard);

        runner.page_turn(true);
        assert_eq!(runner.app().puzzle_index, 1);
        assert_eq!(runner.app().mode, BoardMode::FreeBoard);
        runner.page_turn(false);
        assert_eq!(runner.app().puzzle_index, 0);
        assert_eq!(runner.app().mode, BoardMode::FreeBoard);
    }

    #[test]
    fn promotion_works_without_grading_in_free_board() {
        let (mut runner, _) = runner(Some(PROMOTION_PUZZLES.to_vec()));
        runner.action(action_id(MODE_TOGGLE));

        play(&mut runner, "a7a8r");

        assert_eq!(runner.app().mode, BoardMode::FreeBoard);
        assert_eq!(runner.app().solution_ply, 0);
        assert_eq!(runner.app().solution_feedback, None);
        assert_eq!(
            runner.app().board.piece_at(square_from_name("a8").unwrap()),
            Some(Piece {
                color: Color::White,
                kind: PieceKind::Rook,
            })
        );
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
        runner.action(action_id(&square_action(occupied)));
        assert_ne!(runner.app().board, initial);
        runner.action(action_id(FLIP));
        assert!(runner.app().flipped);
        runner.action(action_id(RESET));
        assert_eq!(runner.app().board, initial);
        assert!(runner.app().flipped, "reset should preserve a manual flip");
        assert_eq!(runner.app().solution_ply, 0);
        assert_eq!(runner.app().solution_feedback, None);
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
    fn startup_lists_collections_and_filters_keys_deterministically() {
        let mut runner = new_runner();
        let commands = runner.start();
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Store(StoreRequest::List))));

        runner.store_result(StoreResult::Keys(vec![
            "notes.json".into(),
            "puzzles-z.json".into(),
            "puzzles-.json".into(),
            PUZZLES_FILE.into(),
            "puzzles-a.json".into(),
            "puzzles-a.json".into(),
        ]));

        assert_eq!(
            runner
                .app()
                .collections
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            vec!["puzzles-a.json", "puzzles-z.json", PUZZLES_FILE]
        );
    }

    #[test]
    fn missing_collections_create_and_load_default_examples() {
        let (runner, commands) = runner(None);
        assert!(commands.iter().any(|command| matches!(
            command, Command::Store(StoreRequest::Save { key, value })
                if key == PUZZLES_FILE && value.as_slice() == EXAMPLE_PUZZLES
        )));
        assert_eq!(
            runner.app().active_collection.as_deref(),
            Some(PUZZLES_FILE)
        );
        assert_eq!(runner.app().puzzles.len(), 10);
        assert!(runner.app().file_error.is_none());
    }

    #[test]
    fn invalid_only_collection_is_preserved_and_examples_are_used_in_memory() {
        let (runner, commands) = runner(Some(b"invalid JSON".to_vec()));
        assert!(runner.app().file_error.is_some());
        assert_eq!(runner.app().puzzles.len(), 10);
        assert!(runner.app().active_collection.is_none());
        assert_eq!(runner.app().collections.len(), 1);
        assert!(!runner.app().collections[0].valid);
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::Store(StoreRequest::Save { .. }))));
    }

    #[test]
    fn picker_uses_title_and_switches_to_a_valid_collection() {
        let (mut runner, _) = runner_with_collections(vec![
            (PUZZLES_FILE, EXAMPLE_PUZZLES.to_vec()),
            ("puzzles-endgames.json", ALT_PUZZLES.to_vec()),
        ]);
        assert_eq!(
            runner.app().active_collection.as_deref(),
            Some(PUZZLES_FILE)
        );
        let initial_board = runner.app().board.clone();

        let index = runner
            .app()
            .collections
            .iter()
            .position(|entry| entry.key == "puzzles-endgames.json")
            .unwrap();
        assert_eq!(
            collection_display_name(&runner.app().collections[index]),
            "Endgames"
        );

        runner.action(action_id(PUZZLE_PICKER));
        assert!(runner.app().picker_open);
        let commands = runner.action(action_id(&collection_action(index)));
        assert!(!runner.app().picker_open);
        assert_eq!(runner.app().board, initial_board);
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(StoreRequest::Load { key }) if key == "puzzles-endgames.json"
        )));

        runner.store_result(StoreResult::Loaded {
            key: "puzzles-endgames.json".into(),
            value: Some(ALT_PUZZLES.to_vec()),
        });

        assert_eq!(
            runner.app().active_collection.as_deref(),
            Some("puzzles-endgames.json")
        );
        assert_eq!(runner.app().puzzle_index, 0);
        assert_eq!(runner.app().puzzles.len(), 2);
        assert_eq!(
            runner.app().puzzles[0].description.as_deref(),
            Some("Endgame one.")
        );

        runner.page_turn(true);
        assert_eq!(runner.app().puzzle_index, 1);
        runner.page_turn(true);
        assert_eq!(runner.app().puzzle_index, 1);
    }

    #[test]
    fn invalid_selected_collection_keeps_current_board_and_reports_its_name() {
        let broken = b"{ not valid json".to_vec();
        let (mut runner, _) = runner_with_collections(vec![
            (PUZZLES_FILE, EXAMPLE_PUZZLES.to_vec()),
            ("puzzles-broken.json", broken.clone()),
        ]);
        let before = runner.app().board.clone();
        let active = runner.app().active_collection.clone();
        let index = runner
            .app()
            .collections
            .iter()
            .position(|entry| entry.key == "puzzles-broken.json")
            .unwrap();

        runner.action(action_id(PUZZLE_PICKER));
        runner.action(action_id(&collection_action(index)));
        runner.store_result(StoreResult::Loaded {
            key: "puzzles-broken.json".into(),
            value: Some(broken),
        });

        assert_eq!(runner.app().active_collection, active);
        assert_eq!(runner.app().board, before);
        assert!(runner
            .app()
            .file_error
            .as_deref()
            .is_some_and(|error| error.contains("puzzles-broken.json")));
        assert!(!runner.app().collections[index].valid);
    }

    #[test]
    fn missing_progress_starts_clean_and_durable() {
        let (runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));
        assert!(runner.app().progress_ready);
        assert!(runner.app().progress_durable);
        assert!(runner.app().progress.files.is_empty());
        assert_eq!(runner.app().progress.active_file, None);
        assert!(runner.app().progress_warning.is_none());
    }

    #[test]
    fn completing_puzzle_marks_only_that_file_and_reset_free_board_keep_it_solved() {
        let (mut runner, _) = runner_with_collections(vec![
            (PUZZLES_FILE, EXAMPLE_PUZZLES.to_vec()),
            ("puzzles-endgames.json", ALT_PUZZLES.to_vec()),
        ]);
        let puzzle_id = runner.app().puzzles[0].id.clone();

        let commands = play(&mut runner, "d7e8");
        assert!(progress_bytes(&commands).is_some());
        assert!(runner.app().progress.is_solved(PUZZLES_FILE, &puzzle_id));
        assert!(!runner
            .app()
            .progress
            .is_solved("puzzles-endgames.json", &puzzle_id));
        let solved = runner
            .app()
            .progress
            .file(PUZZLES_FILE)
            .unwrap()
            .solved_ids
            .clone();

        ack_progress_save(&mut runner);
        runner.action(action_id(RESET));
        runner.action(action_id(MODE_TOGGLE));
        play(&mut runner, "d7d8");

        assert_eq!(
            runner.app().progress.file(PUZZLES_FILE).unwrap().solved_ids,
            solved
        );
        assert!(runner.app().current_is_solved());
    }

    #[test]
    fn switching_files_restores_each_files_remembered_puzzle() {
        let (mut runner, _) = runner_with_collections(vec![
            (PUZZLES_FILE, EXAMPLE_PUZZLES.to_vec()),
            ("puzzles-endgames.json", ALT_PUZZLES.to_vec()),
        ]);

        runner.page_turn(true);
        assert_eq!(runner.app().puzzle_index, 1);
        ack_progress_save(&mut runner);

        switch_collection(&mut runner, "puzzles-endgames.json", ALT_PUZZLES);
        assert_eq!(runner.app().puzzle_index, 0);
        ack_progress_save(&mut runner);

        runner.page_turn(true);
        assert_eq!(runner.app().puzzle_index, 1);
        ack_progress_save(&mut runner);

        switch_collection(&mut runner, PUZZLES_FILE, EXAMPLE_PUZZLES);
        assert_eq!(runner.app().puzzle_index, 1);
        ack_progress_save(&mut runner);

        switch_collection(&mut runner, "puzzles-endgames.json", ALT_PUZZLES);
        assert_eq!(runner.app().puzzle_index, 1);
    }

    #[test]
    fn restart_restores_active_file_current_puzzle_and_solved_state() {
        let (mut runner, _) = runner_with_collections(vec![
            (PUZZLES_FILE, EXAMPLE_PUZZLES.to_vec()),
            ("puzzles-endgames.json", ALT_PUZZLES.to_vec()),
        ]);

        play(&mut runner, "d7e8");
        ack_progress_save(&mut runner);
        switch_collection(&mut runner, "puzzles-endgames.json", ALT_PUZZLES);
        ack_progress_save(&mut runner);
        runner.page_turn(true);
        ack_progress_save(&mut runner);

        let bytes = runner.app().progress.to_bytes().unwrap();
        let (restored, _) = runner_with_state(
            vec![
                (PUZZLES_FILE, EXAMPLE_PUZZLES.to_vec()),
                ("puzzles-endgames.json", ALT_PUZZLES.to_vec()),
            ],
            Some(bytes),
        );

        assert_eq!(
            restored.app().active_collection.as_deref(),
            Some("puzzles-endgames.json")
        );
        assert_eq!(restored.app().puzzle_index, 1);
        assert!(restored
            .app()
            .progress
            .is_solved(PUZZLES_FILE, "lichess-001cr"));
    }

    #[test]
    fn missing_remembered_file_and_puzzle_fall_back_safely() {
        let mut progress = Progress::new();
        progress.active_file = Some("puzzles-gone.json".into());
        progress.remember_puzzle(PUZZLES_FILE, "removed-puzzle");
        let (runner, _) = runner_with_state(
            vec![
                (PUZZLES_FILE, EXAMPLE_PUZZLES.to_vec()),
                ("puzzles-endgames.json", ALT_PUZZLES.to_vec()),
            ],
            Some(progress.to_bytes().unwrap()),
        );
        assert_eq!(
            runner.app().active_collection.as_deref(),
            Some(PUZZLES_FILE)
        );
        assert_eq!(runner.app().puzzle_index, 0);

        let mut progress = Progress::new();
        progress.active_file = Some("puzzles-endgames.json".into());
        progress.remember_puzzle("puzzles-endgames.json", "removed-puzzle");
        let (runner, _) = runner_with_state(
            vec![
                (PUZZLES_FILE, EXAMPLE_PUZZLES.to_vec()),
                ("puzzles-endgames.json", ALT_PUZZLES.to_vec()),
            ],
            Some(progress.to_bytes().unwrap()),
        );
        assert_eq!(
            runner.app().active_collection.as_deref(),
            Some("puzzles-endgames.json")
        );
        assert_eq!(runner.app().puzzle_index, 0);
    }

    #[test]
    fn corrupt_or_future_progress_is_preserved_without_writes() {
        for bytes in [
            b"not json".to_vec(),
            br#"{"version":2,"active_file":"puzzles.json","files":{}}"#.to_vec(),
        ] {
            let (mut runner, _) =
                runner_with_state(vec![(PUZZLES_FILE, EXAMPLE_PUZZLES.to_vec())], Some(bytes));
            assert!(!runner.app().progress_durable);
            assert!(runner.app().progress_warning.is_some());

            let commands = runner.page_turn(true);
            assert!(progress_bytes(&commands).is_none());
            assert!(!runner.app().progress_dirty);
        }
    }

    #[test]
    fn save_failure_stays_dirty_warns_and_retries_on_next_mutation() {
        let (mut runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));
        let commands = runner.page_turn(true);
        assert!(progress_bytes(&commands).is_some());
        assert!(runner.app().progress_saving.is_some());

        runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Unwritable));
        assert!(runner.app().progress_dirty);
        assert!(runner.app().progress_saving.is_none());
        assert_eq!(
            runner.app().progress_warning.as_deref(),
            Some("Progress not saved")
        );

        let retry = runner.page_turn(true);
        assert!(progress_bytes(&retry).is_some());
        assert!(runner.app().progress_saving.is_some());
    }

    #[test]
    fn solved_marker_and_progress_warning_fit_the_kobo_panel() {
        let (mut runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));
        let id = runner.app().puzzles[0].id.clone();
        runner.app_mut().progress.mark_solved(PUZZLES_FILE, &id);
        runner.app_mut().progress_warning = Some("Progress not saved".into());

        let screen = runner.app().screen();
        let diagnostics =
            screen.diagnostics(&runner.context().metrics(), &Chrome::measuring(false));
        assert!(
            !diagnostics.has_errors(),
            "solved/warning screen: {:?}",
            diagnostics.issues
        );
        assert!(runner.app().current_is_solved());
    }

    #[test]
    fn puzzle_feedback_toolbar_and_promotion_modal_fit_the_kobo_panel() {
        let (mut layout_runner, _) = runner(Some(EXAMPLE_PUZZLES.to_vec()));
        let feedback_states = [
            None,
            Some(SolutionFeedback::Correct),
            Some(SolutionFeedback::Wrong),
            Some(SolutionFeedback::Complete),
        ];
        for index in 0..layout_runner.app().puzzles.len() {
            layout_runner.app_mut().select_puzzle(index);
            for mode in [BoardMode::Solution, BoardMode::FreeBoard] {
                layout_runner.app_mut().mode = mode;
                for feedback in feedback_states {
                    layout_runner.app_mut().solution_feedback = feedback;
                    let screen = layout_runner.app().screen();
                    let diagnostics = screen.diagnostics(
                        &layout_runner.context().metrics(),
                        &Chrome::measuring(false),
                    );
                    assert!(
                        !diagnostics.has_errors(),
                        "puzzle {index}, mode {mode:?}, feedback {feedback:?}: {:?}",
                        diagnostics.issues
                    );
                    let feedback_mark =
                        diagnostics.layout.nodes.iter().find(|node| {
                            matches!(node.kind, kobo_sdk::LayoutKind::ChessFeedback(_))
                        });
                    match (mode, feedback) {
                        (BoardMode::Solution, Some(SolutionFeedback::Wrong)) => {
                            assert!(matches!(
                                feedback_mark.map(|node| node.kind),
                                Some(kobo_sdk::LayoutKind::ChessFeedback(Glyph::ThumbDown))
                            ));
                        }
                        (BoardMode::Solution, Some(SolutionFeedback::Complete)) => {
                            assert!(matches!(
                                feedback_mark.map(|node| node.kind),
                                Some(kobo_sdk::LayoutKind::ChessFeedback(Glyph::ThumbUp))
                            ));
                        }
                        _ => assert!(feedback_mark.is_none()),
                    }
                    if let Some(mark) = feedback_mark {
                        let board = diagnostics
                            .layout
                            .nodes
                            .iter()
                            .find_map(|node| match node.kind {
                                kobo_sdk::LayoutKind::ChessFrame { board, .. } => Some(board),
                                _ => None,
                            })
                            .expect("chess board frame");
                        assert!(
                            (mark.rect.x + mark.rect.width / 2 - board.x - board.width / 2).abs()
                                <= 1
                        );
                        assert!(
                            (mark.rect.y + mark.rect.height / 2 - board.y - board.height / 2).abs()
                                <= 1
                        );
                        assert!((mark.rect.width * 4 - board.width).abs() <= 4);
                    }
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
                    let mode_cell = diagnostics
                        .layout
                        .nodes
                        .iter()
                        .find(|node| {
                            matches!(node.kind, kobo_sdk::LayoutKind::Cell(action, _, _)
                                if action == action_id(MODE_TOGGLE))
                        })
                        .expect("mode toggle should have a cell");
                    assert!(matches!(
                        mode_cell.kind,
                        kobo_sdk::LayoutKind::Cell(_, _, selected)
                            if selected == (mode == BoardMode::FreeBoard)
                    ));
                    for action in [MODE_TOGGLE, RESET, FLIP] {
                        let button = rect_for(action_id(action));
                        assert!(
                            (button.width - button.height).abs() <= 1,
                            "button should be square"
                        );
                    }
                    for action in [MODE_TOGGLE, PUZZLE_PICKER] {
                        rect_for(action_id(action));
                    }
                }
            }
        }

        let (mut promotion_runner, _) = runner(Some(PROMOTION_PUZZLES.to_vec()));
        let movement = parse_uci_move("a7a8q").unwrap();
        promotion_runner.action(action_id(&square_action(movement.from)));
        promotion_runner.action(action_id(&square_action(movement.to)));
        let screen = promotion_runner.app().screen();
        let diagnostics = screen.diagnostics(
            &promotion_runner.context().metrics(),
            &Chrome::measuring(false),
        );
        assert!(
            !diagnostics.has_errors(),
            "promotion modal: {:?}",
            diagnostics.issues
        );
        for action in [
            PROMOTE_QUEEN,
            PROMOTE_ROOK,
            PROMOTE_BISHOP,
            PROMOTE_KNIGHT,
            PROMOTION_CANCEL,
        ] {
            assert!(
                diagnostics
                    .layout
                    .nodes
                    .iter()
                    .any(|node| node.kind.acts_on() == Some(action_id(action))),
                "{action} should have a touch target"
            );
        }

        let (mut picker_runner, _) = runner_with_collections(vec![
            (PUZZLES_FILE, EXAMPLE_PUZZLES.to_vec()),
            ("puzzles-endgames.json", ALT_PUZZLES.to_vec()),
        ]);
        picker_runner.action(action_id(PUZZLE_PICKER));
        let screen = picker_runner.app().screen();
        let diagnostics = screen.diagnostics(
            &picker_runner.context().metrics(),
            &Chrome::measuring(false),
        );
        assert!(
            !diagnostics.has_errors(),
            "collection picker: {:?}",
            diagnostics.issues
        );
        for index in 0..picker_runner.app().collections.len() {
            assert!(diagnostics
                .layout
                .nodes
                .iter()
                .any(|node| { node.kind.acts_on() == Some(action_id(&collection_action(index))) }));
        }
        assert!(diagnostics
            .layout
            .nodes
            .iter()
            .any(|node| { node.kind.acts_on() == Some(action_id(PUZZLE_PICKER_CANCEL)) }));
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
