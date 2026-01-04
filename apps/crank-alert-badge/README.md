# CrankAlertBadge

Minimal macOS menu bar badge for Crank alerts.

## Build

```bash
cd apps/crank-alert-badge
swift build -c release
```

## Run

```bash
./.build/release/CrankAlertBadge
```

## Configuration

- `CRANK_ALERTS_DIR`: override alerts directory (default: `~/.crank/alerts`)
