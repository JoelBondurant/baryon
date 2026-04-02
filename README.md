# Baryon

Baryon is an experimental modal terminal editor built around a sparse virtual projection pipeline. The project is currently focused on making large text navigation, byte-accurate editing, and Rust-aware highlighting behave correctly under real editing pressure.

At this stage, Baryon is best thought of as a working editor prototype rather than a complete Vim or NeoVim replacement. It already supports everyday buffer editing, search, substitute, undo/redo, mouse-driven cursor placement, and Rust lexical plus semantic colorization.

## Current State

- Modal TUI frontend built with `ratatui` and `crossterm`
- File open/save flows via CLI argument and ex-style commands
- Normal, Insert, Command, Search, and Confirm modes
- Vim-like navigation with `h`, `j`, `k`, `l`, `gg`, `G`, arrow keys, mouse clicks, and wheel scroll
- Insert and backspace editing with undo on `u` and redo on `Ctrl-r`
- Search with `/pattern`, then `n` and `N`
- Substitute commands for current line, explicit ranges, and whole-file replacement
- Linewise yank/put with unnamed register, named registers, and system clipboard support
- Rust lexical highlighting plus asynchronous semantic highlighting
- Regression tests around tab-aware cursor math, undo/highlight state, document rewrites, and EOF paste behavior

## Running

```bash
cargo run -- path/to/file.rs
```

If no path is provided, Baryon starts without loading an initial file.

Useful development commands:

```bash
cargo test
cargo check
```

## Editor Commands

Normal mode:

- `i` enters insert mode
- `u` undo
- `Ctrl-r` redo
- `yy` yank current line
- `p` put yanked text below the current line
- `"` selects a register prefix, for example `"ayy`, `"ap`, `"+yy`, `"+p`
- `/` starts search
- `:` opens the command line

Command mode:

- `:w` write current file
- `:w path` write to a new path
- `:x` or `:wq` write and quit
- `:q` quit
- `:e path` open another file
- `:42` jump to line 42
- `:s/foo/bar/` substitute on the current line
- `:%s/foo/bar/g` substitute across the whole file
- `:1,20s/foo/bar/c` ranged substitute with confirmation

## Architecture

The project is organized as a layered editor pipeline rather than a monolithic text buffer:

- `src/core/`: shared primitives such as typed coordinates and path helpers
- `src/ecs/`: raw node storage, chunk allocation, and registry-level data access
- `src/svp/`: sparse virtual projection infrastructure, ingestion, resolver work, parsing, highlighting, and semantic analysis
- `src/uast/`: logical tree topology, metrics, mutation helpers, and viewport projection
- `src/engine/`: background command loop, undo ledger, search/substitute logic, clipboard integration, and semantic highlight orchestration
- `src/ui/`: terminal frontend, input handling, rendering, and mouse interaction
- `src/app.rs`: thread wiring, channel setup, and terminal lifecycle

Recent work has been focused on hardening the boundaries between coordinate spaces such as document bytes, document lines, visual columns, and async editor state IDs. That has directly reduced bugs around undo, syntax coloring, tab handling, and mouse/cursor alignment.

## Limitations

- Rust is the only language with semantic highlighting today
- The editing model is intentionally narrower than Vim or NeoVim
- Features like splits, multiple visible buffers, macros, and a full operator/text-object grammar are not implemented
- The codebase is still evolving quickly, especially around typed coordinate boundaries and render/highlight plumbing

## Platform Notes

Baryon currently targets a Linux-style terminal workflow and includes `io_uring` in its storage/ingestion layer. It is being developed primarily as a systems-oriented editor experiment, so the implementation is optimized for clarity of internal boundaries more than broad platform polish at this stage.
