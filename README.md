# spelaiondb-plugin-duckdb

DuckDB for [SpelaionDB](https://github.com/spelaion-dev/spelaiondb), the desktop database
client: the engine's driver, served to SpelaionDB as a plugin.

SpelaionDB's engines are programs it starts and talks to over their standard input and output
(its ADR-041). This is the one for DuckDB. It is built here, in its own repository, and
SpelaionDB pins by hash the exact release it was tested with (its ADR-042): you do not install
this by hand, it comes with SpelaionDB.

- **Protocol:** [SpelaionDB's plugin protocol](https://github.com/spelaion-dev/spelaiondb/blob/dev/docs/plugin-protocol.md),
  version 1.0. Experimental until SpelaionDB 1.0.
- **Engine:** `duckdb`. DuckDB is compiled in (`duckdb` with `bundled`), the same version on
  every platform.
- **License:** Apache-2.0.

## Working on it

```bash
cargo test
```

runs the type zoo (every DuckDB type against `CAST(… AS VARCHAR)`) and the protocol's
conformance set against this plugin's own program. Nothing external is needed.

To try a build with SpelaionDB, point a development build of it at the program:

```bash
SPELAION_DEV_PLUGIN_DUCKDB=/path/to/target/debug/spelaiondb-plugin-duckdb cargo test -p spelaion-core
```

(in SpelaionDB's repository; a release build ignores that variable).

`AGENTS.md` has the rules. Reports and questions go to
[SpelaionDB's issues](https://github.com/spelaion-dev/spelaiondb/issues).
