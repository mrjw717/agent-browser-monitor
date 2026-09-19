# Agent Browser Monitor

A cross-platform Tauri desktop utility for finding and stopping stale `agent-browser` and Playwright sessions.

## Download and install an RC build

1. Open the [Releases page](https://github.com/mrjw717/agent-browser-monitor/releases).
2. Choose the newest version marked **Pre-release**.
3. Download the installer that matches your computer:

| Your computer | Download | What to do next |
| --- | --- | --- |
| Windows 10/11 | `.msi` (recommended) | Double-click it, then follow the installer. |
| macOS | `.dmg` | Open it and drag **Agent Browser Monitor** to Applications. |
| Ubuntu, Debian, Pop!_OS, Linux Mint | `.deb` | Double-click it, or run `sudo dpkg -i ./agent-browser-monitor_*.deb`. |
| Other common Linux systems | `.AppImage` | Mark it executable, then double-click it: `chmod +x ./*.AppImage`. |

After installing, launch **Agent Browser Monitor** from your Start menu, Applications folder, or desktop launcher. It does not need a login, an API key, or an internet connection to inspect local browser processes.

> RC builds are prerelease software. Use the session details before pressing **Terminate**, especially if you have browser automation intentionally running.

## Run it

```sh
cd /home/josh/agent-browser-monitor
npm run tauri dev
```

Build an installable package with:

```sh
npm run tauri build
```

## What it detects

- `agent-browser-linux-x64` plus Chrome processes using `/tmp/agent-browser-chrome-*`;
- Playwright runners and browsers using a Playwright temporary user-data directory.

It does **not** list or terminate ordinary Chrome windows. Session commands are redacted in the UI for URLs and common token/key arguments.

## Platform support

| Platform | Discovery | Manual termination | Installer artifact |
| --- | --- | --- | --- |
| Linux | `/proc` process tree and live CPU/RAM/swap | TERM, then KILL | AppImage and `.deb` |
| macOS | native `ps` process inventory | TERM, then KILL | `.dmg` |
| Windows | PowerShell/WMI process inventory | `taskkill /T /F` | `.msi` and NSIS `.exe` |

Host CPU/RAM/swap cards are currently implemented on Linux; every platform shows the scoped session process details, memory, parentage, and termination controls.

## Publishing an RC

Push a tag such as `v0.1.0-rc.1`. The included GitHub Action builds the native installers on Linux, macOS, and Windows and creates a prerelease containing the artifacts. Repository maintainers need only allow GitHub Actions write access to releases.

## Termination behavior

The selected session must still match those local markers at the moment of the click. Unix sends `SIGTERM`, waits 700 ms, and uses `SIGKILL` only on still-existing processes from that verified tree. Windows uses `taskkill` only for that verified tree.
