# Contributing to FlowScale

FlowScale targets Windows 10 and Windows 11. Contributions are dual-licensed under MIT OR Apache-2.0.

## Workspace layout

| Component | Responsibility |
|---|---|
| `apps/flowscale-ui` | React + Astryx desktop UI compiled to static bundle for WebView2 |
| `crates/flowscale-app` | Windows WebView2 binary (`flowscale.exe`), tray icon, dialogs, and IPC dispatch |
| `crates/flowscale-abi` | C ABI enums, `ScalingProfile`, `StatusEvent`, `StatusCallback` |
| `crates/flowscale-config` | `config.ini` & `settings.json` persistence, `Settings.xml` importer, startup registry helpers, pacers |
| `crates/flowscale-overlay` | Overlay & backdrop windows, WinEvent/foreground watchers, `WH_KEYBOARD_LL` hotkey hook, cursor helpers |
| `crates/flowscale-capture` | `CaptureBackend` trait, `FrameRingQueue`, `DxgiDuplicationCapture`, `WgcCapture`, `GdiCapture` |
| `crates/flowscale-gpu` | Compute upscalers, optical-flow interpolation shaders, swapchain presentation, cursor/FPS D2D overlay |
| `crates/flowscale-core` | Session orchestration (`EngineRuntime`) and `cdylib` C ABI exports (`flowscale_core.dll`) |
| `crates/flowscale-cli` | CLI runner and hardware/pacing diagnostics (`flowscale-cli.exe`) |

## Clean-room policy

Every line of Rust, HLSL, TypeScript, and documentation in this repository must be original clean-room work. Never copy proprietary binaries, shader DXBC bytecode, decompiled code, or model weights.

## UI standards

The frontend in `apps/flowscale-ui` uses `@astryxdesign/core` components and design tokens authored in `themes/flowscale/flowscale.theme.ts`. Avoid raw HTML layout wrappers (`div`, `span`), inline `style` objects, or hardcoded colors and pixel spacing outside theme tokens, and validate source and build output with `npx astryx doctor --built .` and `npx astryx doctor --built dist`.

## Verification

Before submitting changes, run the UI checks and the full Rust workspace tests and release build:

```powershell
cd apps/flowscale-ui
npx tsc --noEmit
npm run build
npx astryx doctor --built .
npx astryx doctor --built dist

cd ../..
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test --workspace
cargo build --workspace --release
```
