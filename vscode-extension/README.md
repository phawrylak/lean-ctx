# lean-ctx VS Code Extension

VS Code sidebar extension for [lean-ctx](https://github.com/yvgude/lean-ctx) — the context engineering layer for AI agents.

## Features

- **Dashboard** — Token savings, session stats, and file activity at a glance
- **Knowledge Panel** — Browse decisions, discoveries, blockers, and insights from the current session
- **Repo Map** — Interactive view of the most important files in your project, ranked by relevance
- **Semantic Search** — Search your codebase by meaning, not just text
- **Status Bar** — Live token savings counter with one-click dashboard access
- **Visualizer** — Launch the lean-ctx call graph visualizer

## Prerequisites

- [lean-ctx](https://github.com/yvgude/lean-ctx) installed and available in `PATH`
- VS Code 1.80.0 or later

## Installation

### From Source

```bash
cd vscode-extension
npm install
npm run compile
npx @vscode/vsce package --no-dependencies
code --install-extension lean-ctx-0.1.0.vsix
```

### Development

```bash
cd vscode-extension
npm install
npm run watch
# Press F5 in VS Code to launch Extension Development Host
```

## Configuration

| Setting | Default | Description |
|---------|---------|-------------|
| `leanctx.binaryPath` | `lean-ctx` | Path to the lean-ctx binary |
| `leanctx.refreshInterval` | `30` | Status bar refresh interval (seconds) |

## Commands

| Command | Description |
|---------|-------------|
| `lean-ctx: Semantic Search` | Opens a search input for semantic code search |
| `lean-ctx: Show Repo Map` | Switches to the repo-map tab in the sidebar |
| `lean-ctx: Knowledge Panel` | Switches to the knowledge tab in the sidebar |
| `lean-ctx: Open Visualizer` | Launches the lean-ctx call graph visualizer |
| `lean-ctx: Refresh Dashboard` | Manually refreshes all dashboard data |

## Architecture

```
src/
├── extension.ts          # Entry point: activate/deactivate
├── leanctx.ts            # CLI interface (all lean-ctx communication)
├── commands.ts           # VS Code command handlers
├── statusbar.ts          # Status bar item with auto-refresh
└── sidebar/
    ├── provider.ts       # Webview view provider
    └── panel.html        # Dashboard UI (HTML/CSS/JS)
```

All communication with lean-ctx happens via CLI subprocess calls in `leanctx.ts`.
The sidebar uses a webview with VS Code's native CSS variables for seamless theming.
