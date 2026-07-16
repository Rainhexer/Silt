```
    ░▒▓█  silt  █▓▒░
  sift · settle · clean
```

Terminal-native Linux storage cleaner. Single static Rust binary. Cache
cleanup, disk drill-down, distro-aware cleanup.

## Install

One command, any Linux distro (x86_64 or aarch64), no dependencies:

```bash
curl -fsSL https://raw.githubusercontent.com/FlyvendeMus/Silt/master/install.sh | sh
```

Downloads a ~2MB static binary to `/usr/local/bin` (or `~/.local/bin` if not writable) and makes `silt` available in your terminal.

### From source

```bash
cargo install --git https://github.com/FlyvendeMus/Silt
```

## Use

```bash
silt                              # interactive TUI
silt --json --root /              # headless scan report
silt --yes --all-safe             # clean safe targets
silt --list-targets               # list what can be cleaned
```

Four tabs (Tab/Shift-Tab): **Overview** (ncdu-style drill), **Clean** (curated
targets with risk tiers), **System** (mounts/pressure), **Log** (action log).

Safe targets are bulk-selectable; Caution targets require per-item confirmation.
Vim keys (`j/k/h/l`), 8 live-switchable themes (`t`/`T`).

## Architecture

```
src/
  main.rs       CLI
  app.rs        state + event loop
  scanner/      walker + mounts
  targets/      fixed registry (sole deletion authority)
  ui/           ratatui rendering
```

The target registry is the only source of deletable paths. Never runs as root.
## License

MIT.
