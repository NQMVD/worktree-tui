# 🌳 Worktree TUI

A beautiful terminal interface for managing Git worktrees, designed with warm tones and intuitive interactions.

![Rust](https://img.shields.io/badge/rust-1.70%2B-orange)
![License](https://img.shields.io/badge/license-MIT-blue)

## Features

- **Visual worktree management** - View all worktrees with status, commits, and sync state
- **Quick navigation** - Vim-style keybindings (`j`/`k`, `g`/`G`, number jumps)
- **Git operations** - Create, delete, lock/unlock, pull, push, fetch, merge
- **Shell integration** - `cd` directly into a worktree with Space
- **Search & filter** - Fuzzy search across branches and paths
- **Beautiful UI** - Claude-inspired warm color palette

## Installation

```bash
cargo install --path .
```

This installs two binaries: `worktree-tui` and `wtt` (short alias).

## Usage

Run from any directory inside a Git repository:

```bash
wtt
```

### Shell Integration (Recommended)

To enable changing directories directly into a worktree when pressing Space, add this function to your `~/.zshrc` or `~/.bashrc`:

```bash
# Worktree TUI with cd integration
wt() {
    local tmp cwd
    tmp="$(mktemp -t "wtt-cwd.XXXXXX")"
    wtt --cwd-file="$tmp" "$@"
    if cwd="$(command cat -- "$tmp")" && [ -n "$cwd" ] && [ "$cwd" != "$PWD" ]; then
        builtin cd -- "$cwd"
    fi
    rm -f -- "$tmp"
}
```

Then reload your shell:

```bash
source ~/.zshrc
```

Now use `wt` instead of `wtt` to get the cd functionality.

## Keybindings

### Navigation

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `g` | Go to first |
| `G` | Go to last |
| `1-9` | Jump to item |
| `Ctrl+d` / `Ctrl+u` | Page down/up |

### Actions

| Key | Action |
|-----|--------|
| `Space` | **Change to worktree directory** (requires shell integration) |
| `c` / `a` | Create new worktree |
| `x` / `Del` | Delete worktree |
| `L` | Toggle lock |
| `p` | Pull |
| `P` | Push |
| `F` | Fetch all remotes |
| `m` | Merge branch |
| `r` / `R` | Refresh list |
| `X` | Prune stale worktrees |

### Utilities

| Key | Action |
|-----|--------|
| `y` | Copy path to clipboard |
| `O` | Open in file manager |
| `s` | Cycle sort order (name/status/recent) |
| `t` | Toggle recent commits panel |
| `/` | Search worktrees |
| `?` | Show help |
| `q` / `Esc` | Quit |

## Worktree Organization

New worktrees are created in a sibling directory named `<repo>-worktrees/`:

```
~/projects/
├── myrepo/              # Main repository
└── myrepo-worktrees/    # Worktrees created by wtt
    ├── feature-a/
    └── feature-b/
```

## License

MIT
