# Claude Instructions

## Project Overview

VibeTree (vibe-tree-rs) is a Rust rewrite of a TypeScript/Electron app for parallel AI-assisted development across multiple git worktrees. Native GPU-accelerated terminal app — no webview, no JS.

## Architecture

Cargo workspace in `crates/`:
- `vt-core` — shared types, config
- `vt-git` — git CLI operations
- `vt-pty` — PTY management (portable-pty 0.8)
- `vt-terminal` — alacritty_terminal 0.25 wrapper + glyphon 0.8 GPU text renderer
- `vt-ui` — egui 0.31 panels/widgets (sidebar, menu bar, dialogs)
- `vt-app` — winit 0.30 event loop, wgpu 24, wires everything together
- `vt-embed` — X11 native window embedding (portal feature, currently disabled)

**Rendering**: egui renders chrome (sidebar, menus, dialogs) with transparent central panel. glyphon renders terminal text on top. Mouse events bypass egui for terminal interactions.

**Split scroll view**: when scrolled, top 2/3 shows scrollback history, bottom ~5 lines show live terminal (pinned prompt).

## Dependency Versions (critical — must match wgpu major version)

- wgpu 24, glyphon 0.8, egui/egui-wgpu/egui-winit 0.31, alacritty_terminal 0.25
- glyphon and egui-wgpu MUST use the same wgpu major version
- alacritty_terminal 0.25: uses `drain_on_exit` (not `hold`), `env` field in tty::Options
- egui-wgpu 0.31: requires `render_pass.forget_lifetime()` for `'static` RenderPass

## Build & Run

```bash
cargo run -p vt-app          # debug
cargo run -p vt-app --release # release
```

## Current Feature Status

### Working
- GPU-accelerated window (wgpu + winit + egui)
- Terminal emulation via alacritty_terminal with PTY I/O
- GPU text rendering via glyphon with correct grid positioning
- Keyboard input forwarded to shell
- Mouse wheel scrolling (3 lines per notch)
- Text selection via click-drag with cyan highlight
- Right-click context menu (Copy/Paste/Clear)
- Ctrl+Shift+C/V for copy/paste
- Sidebar with worktree list (fixed width)
- Split scroll view with live section pinned at bottom
- Window resize recalculates terminal dimensions

### Disabled (behind feature flag)
- **Portal feature** (`vt-embed`): X11 native window embedding — detects app launches from terminal, auto-embeds X11 windows in a side panel. Disabled via `const PORTAL_ENABLED: bool = false` in `vt-app/src/app.rs`. Had issues with false positive detection and embedding reliability. Flip to `true` to re-enable.

## Known Pain Points (from development)
- Terminal text positioning required many iterations to get Y coordinates right
- egui can paint over terminal text if render order is wrong
- Selection coordinates: display coords vs grid coords mismatch with alacritty
- Sidebar resize handle can steal mouse events during selection

## GitHub Actions

View failing build logs: `gh api repos/{owner}/{repo}/actions/jobs/{jobId}/logs`

## Pull Requests

After completing any coding task, create a pull request with the changes.

## Platform

Linux/X11. Window embedding uses X11 reparenting.
