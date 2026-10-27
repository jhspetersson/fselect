# Changelog

## [Unreleased]

### Changed

- Matroska and WebM metadata is behind the `mkv` build feature, enabled by default.
- The `update-notifications` build feature is no longer enabled by default.

### Fixed

- `NOT IN` no longer accepts a row when the list or subquery contains a NULL member and nothing matches
- Numeric functions such as `ABS`, `GREATEST` and `HEX` read the byte count of `fsize` instead of its formatted text

## [0.10.4] - 2026-10-23

### Added

- `IS NULL` and `IS NOT NULL` operators.
- `depth` field.
- `CONTAINS_CYRILLIC` function; better Japanese text detection.
- `STRPOS` function, split from `LOCATE`.
- `rlike` as a regex operator alias.
- Encrypted entries in zip archives; more fields available for archived files, with archive paths included in their values.
- `include:` directives in `.hgignore` and `.dockerignore`.
- Function argument counts and literals compared with typed expressions are checked when the query is parsed.
- Integration test suite covering traversal, ignore files, formats, grouping, dates, archives and CLI flags.

### Changed

- Expressions carry static types; caches, sorting, grouping and aggregates work on typed values instead of rendered text.
- `fsize` is numeric: it compares, groups, sorts and aggregates by byte count while printing the formatted size.
- `CURRENT_DATE`, `NOW`, `DATE_ADD`, `DATE_SUB`, `FROM_UNIXTIME`, `LAST_DAY` and `DATE_TRUNC` return datetimes.
- `LEAST` and `GREATEST` operate on strings as well as numbers.
- `xattr()` returns values that are not UTF-8 as `0x..` hex instead of an empty string ([#187](https://github.com/jhspetersson/fselect/issues/187)).
- Unknown columns in `GROUP BY` and `ORDER BY`, ungrouped columns in grouped queries, an empty `IN ()` and an incomplete `WHERE` clause are rejected.
- A literal that can never match a typed expression (`length(name) > 'abc'`) is rejected before the search starts.
- `LIKE ''` matches the empty string.
- Bare words are treated as function calls only for functions that take no arguments.
- Root aliases are case-insensitive; JSON column names are lowercase.
- Search roots defined by subqueries inherit the root options of the parent query; a grouped subquery in `FROM` yields one entry per group.
- Faster sorting, aggregates, subquery caching, BFS traversal with `LIMIT`, deferred evaluation of heavy columns.

### Fixed

- `GROUP BY` merged distinct values that render alike, such as sizes of 10,241 and 10,242 bytes both shown as `10.00KiB`.
- Missing boolean values sorted as `false` instead of last.
- `HAVING`, aggregate aliases and `MIN`/`MAX`/`COALESCE` corner cases.
- Correlated subqueries: caching, nested contexts and the group values they see.
- Symlink handling and ignore rules applied across symlinks.
- SVG dimension detection, MP4 duration and `exif_alt` for negative altitudes.
- Errors no longer break JSON and HTML output or the row count.
- `--help` and `--version` after other arguments; paths containing whitespace; floating point literals and unary minus.
- Windows: datetime formatting, Unix permission fields, extended attributes on musl builds.
- Subquery `OFFSET` applied twice; depth calculation; path under symlinks.

## [0.10.3] - 2026-09-05

### Added

- `HAVING` clause.
- Expression properties; queries with conflicting `AND` branches are rejected.
- More file type properties.
- `LIKE` escapes.
- Tera, peta and exa file size units.

### Changed

- No ANSI colors when output is not a terminal ([#186](https://github.com/jhspetersson/fselect/issues/186)).
- Git last commits resolved in a single history walk; SIMD search in `contains()`.
- Better `.hgignore` and `.dockerignore` fidelity.

### Fixed

- Unary minus handling.
- `EPOCH`, `DATE_DIFF` and `SUBSTRING` semantics; date literals built without a local round-trip.
- Line count, UNC paths, `exif_alt`; CSV output of split UTF-8.
- Subquery caching, `trim`, symlink gates; `NOT EXISTS` without a left operand.
- Audio tag year parsing, sub-second durations, MP4 timescale; SVG units and viewBox fallback.
- Relative day syntax for four-digit values; `.hgignore` glob conversion.
- Empty value instead of an error on math domain errors.

## [0.10.2] - 2026-06-28

### Added

- Index-backed search with `plocate` and (experimental) Everything ([#98](https://github.com/jhspetersson/fselect/issues/98)).
- Git-related fields.
- `word_count`, `char_count`, `encoding`, `has_bom`, `line_ending` fields.
- `symlink_target` and `is_broken_symlink` fields.
- `blksize`, `atime_nsec`, `mtime_nsec`, `ctime_nsec` fields.
- `mp3_comment`, `mp3_track`, `mp3_disc` fields; metadata extraction for FLAC, Ogg Vorbis, Opus.
- `default_acl`, `acl_entry` and extattrs on Windows.
- `.qml` as a known source extension.

### Changed

- Fewer syscalls per file ([#175](https://github.com/jhspetersson/fselect/issues/175)).
- Content stats are streamed instead of reading whole files.
- `IN` with a multi-column subquery fails, as in SQL.

### Fixed

- Correlated subquery dependency collection.
- Ordering and grouping by complex expressions; `MIN`/`MAX` for datetimes; `COUNT` skipping empty values.
- Exit code when an error occurs during query evaluation.
- Unknown aliases; command-line argument parsing.
- `.dockerignore` and `.hgignore` syntax.

## [0.10.1] - 2026-05-30

### Added

- Subqueries in the `FROM` clause.
- Positional `GROUP BY`.
- `EXTRACT` and `DATE_TRUNC` functions.
- `wasm32-wasip1` target.

### Changed

- `t` and `f` are parsed as boolean values.
- Invalid datetime literals are detected early.
- Release builds use a single codegen unit ([#183](https://github.com/jhspetersson/fselect/issues/183)).

### Fixed

- Panic when parsing invalid datetimes ([#184](https://github.com/jhspetersson/fselect/issues/184)).
- Correlated subqueries referencing outer aliases were cached.
- JSON output column order, duplicates and duplicate names.
- `CONTAINS` search is streamed, avoiding OOM and the UTF-8 requirement.
- Indented comments in `.dockerignore` and `.hgignore`; REPL `cd` paths with internal whitespace.
- Compilation with musl.

## [0.10.0] - 2026-04-10

### Added

- POSIX ACLs and extended file attributes; Linux capabilities v2 and v3.
- Xattrs and `has_acl` on Windows; ACL formatting on Windows.
- `is_sticky`, `atime`, `mtime`, `ctime`, `rdev`, `xattr_count` fields.
- EXIF fields: `exif_datetime_original`, `exif_description`, `exif_artist`, `exif_copyright`, `exif_orientation`, `exif_flash`, `exif_color_space`, `exif_sensitivity` and more.
- More datetime functions; a precision parameter for `ROUND`.
- Zero-argument functions can be called without parentheses.
- `SearchError` with error levels; the search stops on fatal errors.

### Changed

- ACL, default ACL and capability fields moved from functions to fields.
- Streaming accumulators for aggregates; row separators in grouped output.
- Lexer and parser rewritten for correctness and speed.

### Fixed

- Many parser, lexer, function and traversal bugs found by a systematic review.
- `NO_COLOR` handling; `is_symlink`; symlink resolution during traversal.
- `.hgignore` and `.dockerignore` pattern escaping and negation.
- `OFFSET` for non-buffered queries; `LIMIT` + `OFFSET` overflow.
- HTML output escaping; config path forced to lowercase.
- EXIF GPS rational values and division by zero.

[Unreleased]: https://github.com/jhspetersson/fselect/compare/0.10.4...HEAD
[0.10.4]: https://github.com/jhspetersson/fselect/compare/0.10.3...0.10.4
[0.10.3]: https://github.com/jhspetersson/fselect/compare/0.10.2...0.10.3
[0.10.2]: https://github.com/jhspetersson/fselect/compare/0.10.1...0.10.2
[0.10.1]: https://github.com/jhspetersson/fselect/compare/0.10.0...0.10.1
[0.10.0]: https://github.com/jhspetersson/fselect/compare/0.9.3...0.10.0
