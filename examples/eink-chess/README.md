# Example puzzles

`src/puzzles.json` contains ten puzzles from the
[Lichess open puzzle database](https://database.lichess.org/#puzzles), released
under **CC0-1.0**. Downloaded on 2026-09-27 from the export dated 2026-09-10.
They may be copied, modified, and redistributed freely.

The examples alternate between White and Black to move. They include two
mates in one, two mates in two, and six puzzles asking for the best
continuation. Each puzzle retains its Lichess ID and source link. The short
instructions were added for this app.

`lichess-source.csv` preserves the ten original database rows. To produce
`src/puzzles.json`, each CSV row's first UCI move was applied to its FEN with
python-chess, and the resulting six-field FEN was saved. The remaining UCI
moves became the solution array. All setup and solution moves were checked
for legality; the four mating lines were checked to finish in checkmate.
python-chess is only a data preparation tool, not an app dependency.

See [the puzzle format](https://github.com/abavelski/eink-chess-app/blob/master/docs/PUZZLE_FORMAT.md) for editing and installation.
This example is copied from the E-Ink Chess app by its `scripts/prepare_cobalt.py`.
The older `positions.fen` file is no longer loaded.
