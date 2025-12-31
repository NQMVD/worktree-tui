# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Rust TUI application called `worktree-tui` (binary alias: `wtt`) - a terminal interface for managing Git worktrees with visual presentation and intuitive navigation. The project uses the `ratatui` framework and `gix` (gitoxide) for Git operations.

**Note:** This repository also contains an autonomous agent testing infrastructure. When working in agent mode (via `tui_agent_loop.sh`), strict rules apply - see README_AGENT.md for details.

## Development Commands

```bash
# Build the application
cd worktree-tui
cargo build

# Release build (with LTO optimizations)
cargo build --release

# Install locally (creates both worktree-tui and wtt binaries)
cargo install --path .

# Run checks (faster than full build)
cargo check

# Lint with Clippy
cargo clippy

# Format code
cargo fmt
```

## Testing the TUI

The `fifty-worktrees/` directory contains a test repository with 50 worktrees for comprehensive testing.

```bash
# Start TUI in test repository
cd fifty-worktrees
../worktree-tui/target/debug/worktree-tui

# Or use tmux bridge for automated testing
./tmux_bridge.sh start wtt "../worktree-tui/target/debug/worktree-tui"
```

### tmux_bridge Commands

The `tmux_bridge.sh` script manages tmux sessions for automated TUI testing:

| Command | Usage |
|---------|-------|
| `./tmux_bridge.sh start <name> "<cmd>" [w] [h]` | Start app in named session |
| `./tmux_bridge.sh inspect <name>` | Get cursor position and screen text |
| `./tmux_bridge.sh screenshot <name> [file]` | Capture PNG to screenshots/ |
| `./tmux_bridge.sh send <name> "<keys>"` | Send keystrokes (Up, Down, Enter, etc.) |
| `./tmux_bridge.sh stop <name>` | Kill session and save log to logs/ |

## Architecture

### Core Structure

The application is primarily contained in two files:

- **`worktree-tui/src/main.rs`** (~27,000 lines) - All UI rendering, event handling, keybindings, and state management
- **`worktree-tui/src/cache.rs`** - Serializable JSON cache with 10-second TTL for instant startup

### Key Dependencies

- **ratatui** - Terminal UI framework
- **gix** (gitoxide) - Git operations (replaces libgit2)
- **tokio** - Async runtime for Git operations
- **crossterm** - Terminal and event handling

### Application Modes

The TUI operates in different modes (Normal, Help, Create, Delete, Search, etc.) with state-driven rendering. Key features include:

- Vim-style navigation (j/k, g/G, number jumps)
- Fuzzy search (`/`)
- Git operations (create, delete, lock/unlock, pull, push, fetch, merge)
- Shell integration (`cd` via `--cwd-file` for the `wt()` shell function)
- Sort options (name, status, recent activity)
- Recent commits panel (toggle with `t`)

### Cache System

Worktree data is cached in `~/.cache/wtt/` as JSON with a 10-second TTL. This enables instant app startup while refreshing data in the background.

## Autonomous Agent Rules

When running via `tui_agent_loop.sh` (as an autonomous agent), additional rules from README_AGENT.md apply:

1. **No path access outside CWD** - Strict jail at `/Users/noah/dev/vibesites/worktree-testing/autonomous`
2. **No `git push`** - All work stays local
3. **Use tmux_bridge** - All TUI interactions must go through the bridge, not direct process access
4. **Maintain agent_worklog.md** - Persistent memory for task tracking
5. **Atomic commits** - Commit after each verified sub-task, no history rewrites

The agent operates on an O.P.E.V. cycle: Observe → Plan → Execute → Verify → Commit.

## Known Issues

Per HUMAN.md:
- Path permission restrictions when agent accesses outside directories
- tmux sessions not automatically cleaned up (agents quit the app but don't kill sessions)
- No commits being made during agent development sessions
