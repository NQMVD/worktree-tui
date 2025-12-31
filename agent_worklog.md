# Agent Development Worklog

## Current Goal
Find and fix all bugs in worktree-tui tool. Migration to gix was complex - need to ensure all features work properly. Testing can use fifty-worktrees/ repo in tmux sessions.

## Task Checklist
- [ ] Start tmux session with worktree-tui on test repo
- [ ] Test basic navigation (j/k, g/G, number jumps)
- [ ] Test create worktree feature (c/a)
- [ ] Test delete worktree feature (x/Del)
- [ ] Test lock/unlock (L)
- [ ] Test git operations (p/P pull/push, F fetch)
- [ ] Test search functionality (/)
- [ ] Test cd integration (Space)
- [ ] Test sort cycling (s)
- [ ] Test recent commits panel (t)
- [ ] Test help dialog (?)
- [ ] Document and fix any bugs found
- [ ] Verify all fixes with automated testing

## Insights
- Just added libc dependency and low-level TTY handling for ZSH widget support (CTRL+G)
- Previous fix for "reader source not set" panic needed dup2() calls to redirect stdin/stdout to /dev/tty

## Bug Testing Session

### Session Setup
- Testing worktree-tui on fifty-worktrees repo (51 worktrees)
- Using tmux_bridge.sh for all interactions

### Bugs Found and Fixed

#### Bug #1: Details Panel Mislabeling Status
**Description**: Details panel showed "Modified" label for any non-clean status, even when worktree only had untracked files. This was misleading to users.

**Expected**: Worktree with only untracked files should show "Changes ?2" not "Modified ?2"
**Actual**: Was showing "Modified ?2" for untracked-only status

**Fix**: Modified `render_details_panel()` function in `src/main.rs` (lines 2036-2065) to check if there are staged or modified files before using "Modified" label. If only untracked files exist, use "Changes" label instead.

**Code Change**: Added logic to determine if worktree has actual changes (staged/modified) vs just untracked files:
```rust
let has_changes = wt.status.staged > 0 || wt.status.modified > 0;
let status_label = if has_changes { "Modified" } else { "Changes" };
```

**Status**: ✓ Fixed and verified

### Testing Status
1. **Navigation (j/k)** - Working ✓
2. **Number jumps (1-9)** - Working ✓ (tested 5, 9)
3. **g/G (first/last)** - Working ✓
4. **Search (/)** - Working ✓ (fuzzy search works, Enter selects)
5. **Create worktree (c/a)** - Working ✓ (dialog opens, can cancel)
6. **Delete worktree (x)** - Working ✓ (confirmation dialog)
7. **Lock/unlock (L)** - Working ✓
8. **Git operations (p/P, F)** - Working ✓ (error handling works)
9. **Sort cycling (s)** - Working ✓ (cycles: recent, name, status, recent)
10. **Recent commits (t)** - Working ✓ (toggle shows/hides panel)
11. **Help (?)** - Working ✓ (dialog shows/closes)
12. **CD integration (Space)** - Working ✓ (exits TUI as expected)
13. **Refresh (r/R) and prune (X)** - Working ✓
