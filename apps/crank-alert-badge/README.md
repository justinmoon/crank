# CrankAlertBadge

Minimal macOS menu bar badge for Crank alerts.

## Build

```bash
cd apps/crank-alert-badge
swift build -c release
```

## Build app bundle

```bash
cd apps/crank-alert-badge
./build.sh
```

## Run

```bash
./.build/release/CrankAlertBadge
```

## Install

```bash
cd apps/crank-alert-badge
./build.sh
cp -r CrankAlertBadge.app /Applications/
```

## Configuration

- `CRANK_ALERTS_DIR`: override alerts directory (default: `~/.crank/alerts`)
