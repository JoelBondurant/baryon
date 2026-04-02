## Baryon

Baryon is a modal terminal editor built around a sparse virtual projection pipeline. The current focus is dogfooding: making large-file navigation, byte-accurate editing, visual feedback, and Rust-aware highlighting hold up under real daily use.

It is not a full Vim or NeoVim replacement yet, but it is well past toy status. Baryon now supports real editing sessions with modal input, search and substitute, undo/redo, visual selections, Rust lexical plus semantic colorization, and a threaded right-hand minimap.

### Current State

- Modal TUI frontend built with `ratatui` and `crossterm`
- File open/save flows via CLI argument, ex-style commands, and Debian package install support
- Normal, Insert, Command, Search, Confirm, and Visual modes
- Vim-like navigation with `h`, `j`, `k`, `l`, `w`, `b`, `e`, `gg`, `G`, `0`, `^`, `$`, `Home`, `End`, `PageUp`, `PageDown`, arrow keys, mouse clicks, and wheel scroll
- Insert and backspace editing with sparse in-place mutation, undo on `u`, and redo on `Ctrl-r`
- Search with `/pattern`, then `n` and `N`
- Substitute commands for current line, explicit ranges, whole-file replacement, and interactive confirm flows
- Visual char, line, and block selection with yank, delete, and change
- Structural delete/change motions including `diw`, `ciw`, and `D`
- Linewise yank/put with unnamed register, named registers, and system clipboard support
- Rust lexical highlighting plus asynchronous semantic highlighting
- Threaded protocol-aware right-hand minimap with viewport and search markers
- Streaming save path with atomic temp-file writes and sparse rebind after save
- Regression tests around tab-aware cursor math, undo/highlight state, sparse mutation, visual range math, minimap search bands, and EOF paste behavior

### Running

```bash
cargo run -- path/to/file.rs
```

If no path is provided, Baryon starts without loading an initial file.

Useful development commands:

```bash
cargo test
cargo check
```

### Installing

If `cargo deb --install` is configured in your environment, Baryon can be installed directly:

```bash
cargo deb --install
```

On systems where you want Baryon available as `vim`, `vi`, or the generic `editor`, register it with `update-alternatives`:

```bash
sudo update-alternatives --install /usr/bin/vim vim /usr/bin/baryon 100
sudo update-alternatives --config vim

sudo update-alternatives --install /usr/bin/vi vi /usr/bin/baryon 100
sudo update-alternatives --config vi

sudo update-alternatives --install /usr/bin/editor editor /usr/bin/baryon 100
sudo update-alternatives --config editor
```

### Editor Commands

Normal mode:

- `i` enters insert mode
- `u` undo
- `Ctrl-r` redo
- `yy` yank current line
- `p` put yanked text below the current line
- `"` selects a register prefix, for example `"ayy`, `"ap`, `"+yy`, `"+p`
- `/` starts search
- `:` opens the command line
- `v`, `V`, and `Ctrl-v` enter visual char, line, and block mode
- `diw`, `ciw`, and `D` are implemented

Visual mode:

- `y` yank selection
- `d` delete selection
- `c` change selection and enter insert mode
- movement keys extend the active selection

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

### Architecture

The project is organized as a layered editor pipeline rather than a monolithic text buffer:

- `src/core/`: shared primitives such as typed coordinates and path helpers
- `src/ecs/`: raw node storage, chunk allocation, and registry-level data access
- `src/svp/`: sparse virtual projection infrastructure, ingestion, resolver work, parsing, highlighting, and semantic analysis
- `src/uast/`: logical tree topology, metrics, mutation helpers, and viewport projection
- `src/engine/`: background command loop, undo ledger, search/substitute logic, clipboard integration, and semantic highlight orchestration
- `src/ui/`: terminal frontend, input handling, rendering, and mouse interaction
- `src/app.rs`: thread wiring, channel setup, and terminal lifecycle

Recent work has been focused on hardening the boundaries between coordinate spaces such as document bytes, document lines, visual columns, and async editor state IDs. That has directly reduced bugs around undo, syntax coloring, tab handling, and mouse/cursor alignment.

### Limitations

- Rust is the only language with semantic highlighting today
- The editing model is still intentionally narrower than Vim or NeoVim
- Features like splits, multiple visible buffers, macros, and a full operator/text-object grammar are not implemented
- Some everyday editing edges are still rough during dogfooding, especially outside the currently-covered modal/navigation/search flows
- The codebase is still evolving quickly, especially around typed coordinate boundaries and render/highlight plumbing

### Platform Notes

Baryon currently targets a Linux-style terminal workflow and includes `io_uring` in its storage/ingestion layer. It is being developed primarily as a systems-oriented editor experiment, so the implementation is optimized for clarity of internal boundaries more than broad platform polish at this stage.

### Screenshots
![Baryon](https://github.com/JoelBondurant/baryon/blob/main/img/screenshot_1.png?raw=true)