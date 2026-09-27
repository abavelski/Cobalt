//! Versioned puzzle files. The FEN active color is the side solving the puzzle.

use crate::board::{Board, Color};
use serde::Deserialize;
use std::collections::HashSet;

pub const MAX_PUZZLE_FILE_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
struct PuzzleFile {
    version: u32,
    puzzles: Vec<Puzzle>,
}

#[derive(Debug, Deserialize)]
pub struct Puzzle {
    pub id: String,
    pub fen: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Alternating player and opponent moves, starting with the player's move.
    pub solution: Vec<String>,
}

impl Puzzle {
    pub fn side_to_move(&self) -> Color {
        match self.fen.split_ascii_whitespace().nth(1) {
            Some("b") => Color::Black,
            _ => Color::White,
        }
    }
}

pub fn parse_puzzle_file(contents: &[u8]) -> Result<Vec<Puzzle>, String> {
    if contents.len() > MAX_PUZZLE_FILE_BYTES {
        return Err("puzzles.json must be at most 256 KiB.".into());
    }
    let file: PuzzleFile = serde_json::from_slice(contents)
        .map_err(|error| format!("Invalid puzzles.json: {error}"))?;
    if file.version != 1 {
        return Err(format!(
            "Unsupported puzzle file version {}; expected 1.",
            file.version
        ));
    }
    if file.puzzles.is_empty() {
        return Err("No puzzles found. Add at least one puzzle to puzzles.json.".into());
    }
    let mut ids = HashSet::new();
    for (index, puzzle) in file.puzzles.iter().enumerate() {
        let name = format!("Puzzle {} ({})", index + 1, puzzle.id);
        if puzzle.id.trim().is_empty() || !ids.insert(puzzle.id.as_str()) {
            return Err(format!("{name}: id must be nonempty and unique."));
        }
        Board::from_fen(&puzzle.fen).map_err(|error| format!("{name}: {error}"))?;
        if puzzle.solution.is_empty() {
            return Err(format!(
                "{name}: solution must contain at least one UCI move."
            ));
        }
        for (move_index, movement) in puzzle.solution.iter().enumerate() {
            if !is_uci_move(movement) {
                return Err(format!(
                    "{name}: solution move {} must use UCI notation (for example e2e4 or a7a8q).",
                    move_index + 1
                ));
            }
        }
    }
    Ok(file.puzzles)
}

fn is_uci_move(movement: &str) -> bool {
    let bytes = movement.as_bytes();
    (bytes.len() == 4 || bytes.len() == 5)
        && matches!(bytes[0], b'a'..=b'h')
        && matches!(bytes[1], b'1'..=b'8')
        && matches!(bytes[2], b'a'..=b'h')
        && matches!(bytes[3], b'1'..=b'8')
        && bytes[..2] != bytes[2..4]
        && (bytes.len() == 4 || matches!(bytes[4], b'q' | b'r' | b'b' | b'n'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> serde_json::Value {
        json!({"version": 1, "puzzles": [{
            "id": "sample", "fen": "7k/8/5KQ1/8/8/8/8/8 w - - 0 1",
            "solution": ["g6g7"]
        }]})
    }

    fn parse(value: serde_json::Value) -> Result<Vec<Puzzle>, String> {
        parse_puzzle_file(&serde_json::to_vec(&value).unwrap())
    }

    #[test]
    fn accepts_optional_description_and_derives_turn_from_fen() {
        let puzzles = parse(sample()).unwrap();
        assert!(puzzles[0].description.is_none());
        assert_eq!(puzzles[0].side_to_move(), Color::White);
        let mut value = sample();
        value["puzzles"][0]["fen"] = json!("8/8/8/8/8/5kq1/8/7K b - - 0 1");
        value["puzzles"][0]["description"] = json!("Find a mate in one.");
        value["puzzles"][0]["solution"] = json!(["g3g2"]);
        let puzzles = parse(value).unwrap();
        assert_eq!(puzzles[0].side_to_move(), Color::Black);
        assert_eq!(
            puzzles[0].description.as_deref(),
            Some("Find a mate in one.")
        );
    }

    #[test]
    fn rejects_malformed_empty_and_future_files() {
        assert!(parse_puzzle_file(b"not json").is_err());
        assert!(parse(json!({"version": 1, "puzzles": []})).is_err());
        let mut value = sample();
        value["version"] = json!(2);
        assert!(parse(value).unwrap_err().contains("version 2"));
        assert!(parse_puzzle_file(&vec![b' '; MAX_PUZZLE_FILE_BYTES + 1]).is_err());
    }

    #[test]
    fn rejects_invalid_positions_ids_and_solutions() {
        let mut value = sample();
        value["puzzles"][0]["fen"] = json!("invalid");
        assert!(parse(value).unwrap_err().contains("FEN"));
        let mut value = sample();
        value["puzzles"][0]["id"] = json!(" ");
        assert!(parse(value).is_err());
        let mut value = sample();
        let duplicate = value["puzzles"][0].clone();
        value["puzzles"].as_array_mut().unwrap().push(duplicate);
        assert!(parse(value).is_err());
        for solution in [
            json!([]),
            json!(["Qg7#"]),
            json!(["e2e2"]),
            json!(["a7a8k"]),
        ] {
            let mut value = sample();
            value["puzzles"][0]["solution"] = solution;
            assert!(parse(value).is_err());
        }
        assert!(is_uci_move("a7a8q"));
        assert!(is_uci_move("e1g1"));
    }
}
