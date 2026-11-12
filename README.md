# LocalDesk

Локальный удалённый рабочий стол **macOS (Apple Silicon) → Windows 10/11**.
Цель проекта — прямое подключение в LAN без облака, relay-сервисов и обязательного доступа в интернет.

> Текущий статус: ранний инженерный каркас `0.1.0`. Реализованы общие RTP/H.264 примитивы и UDP-приёмник/диагностический отправитель. Захват экрана macOS, аппаратное кодирование, декодирование и удалённый ввод ещё предстоит интегрировать. Это пока не готовая замена VNC.

## Цели

- 1920×1080 при целевых 60 FPS;
- прямой видеотранспорт UDP/RTP;
- отдельный надёжный канал управления;
- аппаратное кодирование/декодирование там, где доступно;
- явная авторизация и шифрование;
- работа в изолированной локальной сети.

## Структура

```text
localdesk/
├── Cargo.toml
├── README.md
├── crates/
│   ├── localdesk-protocol/   # RTP, типы протокола, H.264 packetization
│   └── localdesk-transport/  # UDP socket helpers
├── apps/
│   ├── macos-server/         # будущий macOS host
│   └── windows-client/       # Windows receiver/client
├── docs/
│   ├── ARCHITECTURE.md
│   ├── PROTOCOL.md
│   └── SECURITY.md
└── scripts/
```

## Сборка

Требуется Rust stable и Cargo.

```bash
cargo check --workspace
cargo test --workspace
```

Запустить UDP-приёмник на Windows:

```powershell
cargo run -p localdesk-windows-client -- --listen 0.0.0.0:5004
```

Запустить диагностическую отправку с Mac (пока это тестовые RTP payload, не настоящее видео):

```bash
cargo run -p localdesk-macos-server -- --target 192.168.1.20:5004 --test-packets 300
```

Если Windows Firewall спросит разрешение, разрешить входящий UDP только для частной сети и только от IP Mac. Для диагностики не открывать порты на маршрутизаторе.

## Важные ограничения каркаса

1. UDP/RTP на текущем этапе **не зашифрован**. Использовать только в доверенной тестовой LAN; не передавать реальные изображения/ввод.
2. `--test-packets` отправляет синтетические тестовые данные, а не H.264 bitstream.
3. Порт `5004/UDP` — временный порт лабораторной диагностики, не финальная схема портов.
4. macOS ScreenCaptureKit и VideoToolbox требуют нативной интеграции и разрешений TCC; эти модули ещё не реализованы.
5. Windows hardware decode и renderer пока не реализованы.

## Лицензия

Лицензия проекта пока не выбрана. Перед публичной публикацией нужно явно определить условия распространения и использования.

## Native macOS capture milestone (0.2)

The macOS server now contains a ScreenCaptureKit capture probe. See
[`docs/SCREEN_CAPTURE.md`](docs/SCREEN_CAPTURE.md) for prerequisites, permission
setup, invocation, and cross-compilation notes.

Build and run on Apple Silicon Mac:

```bash
./scripts/build-macos.sh
cargo run -p localdesk-macos-server --target aarch64-apple-darwin -- \
  --width 1920 --height 1080 --fps 60 --seconds 15
```

This milestone validates capture and frame cadence only. VideoToolbox encoding,
RTP transmission, input injection, and the GUI are still subsequent milestones.
