# UI / Engine Map

Короткая карта интерфейса и движков в проекте.

## 1. Роли приложений

### Windows desktop

- `Host mode`
  - захват экрана Windows
  - encode видео
  - отправка в desktop client / Android client
  - lease / control-plane host registration

- `Client mode`
  - receive поток
  - decode видео
  - render в окно
  - managed connect через control-plane

### Android

- `PC receiver mode`
  - connect by short code
  - receive поток от Windows host
  - decode через Android stack
  - relay/direct route managed через control-plane

## 2. UI desktop

Файл:
- [MainForm.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/receiver-native/MainForm.cs)

Главные зоны:

- `Connection`
  - control-plane URL
  - auth
  - simple / advanced mode

- `Connect`
  - short code
  - host list
  - connect / stop / resume

- `Client Settings`
  - playback backend
  - decoder mode
  - transport
  - advanced receiver tuning

- `Host`
  - monitor
  - receiver target
  - preset
  - encoder
  - codec
  - bitrate / fps / resolution

- `HUD`
  - runtime telemetry
  - selected route
  - selected codec
  - auto encoder result

## 3. UI Android

Файл:
- [PcReceiverModeScreen.kt](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/app/src/main/java/com/everty/evertygame/receiver/PcReceiverModeScreen.kt)

Главные зоны:

- connect by short code
- quick presets:
  - `Low Latency`
  - `Balanced`
  - `Quality`
  - `AV1 Max`
- advanced settings
- live logs / diagnostics

Preset сейчас влияет на:
- width
- height
- fps
- bitrate
- preferred codec order

## 4. Windows encode backends

Файл:
- [WindowsSenderSession.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/receiver-native/WindowsSenderSession.cs)

Поддержанные encode path:

- `NVIDIA NVENC`
  - через `ffmpeg`
  - обычно `DDAgrab + nvenc`
  - low-latency preferred path для NVIDIA

- `Intel Quick Sync`
  - через `ffmpeg`
  - low-latency path для Intel iGPU

- `Media Foundation`
  - native Windows encoder path
  - fallback hardware path

- `Software`
  - ffmpeg software encoder
  - последний fallback

### Auto encoder selection

Порядок:

1. `NVIDIA NVENC`
2. `Intel Quick Sync`
3. `Media Foundation`
4. `Software`

Auto probe делает:

- adapter vendor detection
- ffmpeg encoder availability probe
- Media Foundation encoder probe

HUD показывает:

- `Auto encoder`
- `Encoder path`

Разница:

- `Auto encoder`
  - что выбрала логика auto-detect

- `Encoder path`
  - фактический runtime path sender

## 5. Decode / playback backends

### Windows client

Файлы:
- [MediaFoundationD3D11PlaybackController.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/receiver-native/MediaFoundationD3D11PlaybackController.cs)
- [LibVlcPlaybackController.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/receiver-native/LibVlcPlaybackController.cs)

Основной path:

- `Media Foundation + D3D11`
  - preferred desktop playback backend

Доп path:

- `LibVLC`
  - fallback / experimental playback backend

### Android client

Файлы:
- [PcReceiverClientController.kt](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/app/src/main/java/com/everty/evertygame/receiver/PcReceiverClientController.kt)

Decode path:

- `MediaCodec`
- render в `Surface`

Итог:

- Windows host encode != Windows client decode
- `NVENC` только encode
- Android decode никогда не `NVENC`

## 6. Codecs

Файлы:
- [WindowsVideoCodec.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/receiver-native/WindowsVideoCodec.cs)
- [VideoCodec.kt](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/app/src/main/java/com/everty/evertygame/stream/VideoCodec.kt)

Сейчас модель:

- `video/av1`
- `video/hevc`
- `video/avc`

### Negotiation policy

Control-plane выбирает codec так:

1. `AV1`
2. `HEVC`
3. `AVC`

Но только если:

- host умеет encode
- client умеет decode

Иначе fallback вниз по списку.

### AV1

Есть:

- shared codec model
- control-plane negotiation
- Windows sender capability probe
- Windows playback decode wiring
- Android decode capability advertise

Нужно помнить:

- AV1 runtime зависит от реального hardware / OS support
- если hardware AV1 нет, auto policy падает в `HEVC`, потом в `AVC`

## 7. Route model

Главный файл:
- [Program.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/control-plane/Program.cs)

Route kinds:

- `direct_host_push`
  - same LAN direct route

- `direct_punched`
  - direct p2p recovered/punched route

- `relay_assigned`
  - full relay path

- `direct_fallback`
  - degraded / fallback state

### Route priority

- same LAN => prefer direct
- relay => fallback

### Важный нюанс

В client code route split now separate:

- `relay registration route`
  - нужна для регистрации receiver в relay и fallback

- `transport route`
  - куда реально идут control/media

Это важно, чтобы `direct_host_push` не ломался из-за того, что клиент начинал слать control в relay только потому, что relay endpoint был известен как fallback.

Файлы:
- [NativeReceiverSession.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/receiver-native/NativeReceiverSession.cs)
- [PcReceiverClientController.kt](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/app/src/main/java/com/everty/evertygame/receiver/PcReceiverClientController.kt)

## 8. Control-plane capability exchange

Host capabilities:

- supported encode codecs
- supported decode codecs
- supported encoder backends
- LAN addresses

Client capabilities:

- supported decode codecs
- LAN addresses

Create session request now can carry:

- `preferredCodecs`
- `presetId`
- `capabilities`

Файлы:
- [ControlPlaneClient.kt](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/app/src/main/java/com/everty/evertygame/receiver/ControlPlaneClient.kt)
- [DesktopControlPlaneClient.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/receiver-native/DesktopControlPlaneClient.cs)
- [ControlPlaneAgent.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/receiver-native/ControlPlaneAgent.cs)
- [Program.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/control-plane/Program.cs)

## 9. Latency model

Low-latency bias сейчас делается через:

- quick presets
- low-latency sender preset `Game`
- startup keyframe request burst
- reduced reconnect wait
- telemetry-driven route fallback / recovery
- HUD metrics:
  - `Pulse -> Android`
  - `Input -> Android`
  - `Receiver decode`
  - `Receiver d/p ms`

## 10. Что где менять

Если надо менять desktop UI:
- [MainForm.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/receiver-native/MainForm.cs)

Если надо менять Android receiver UX:
- [PcReceiverModeScreen.kt](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/app/src/main/java/com/everty/evertygame/receiver/PcReceiverModeScreen.kt)

Если надо менять Android transport / decoder:
- [PcReceiverClientController.kt](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/app/src/main/java/com/everty/evertygame/receiver/PcReceiverClientController.kt)

Если надо менять Windows sender / encoder selection:
- [WindowsSenderSession.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/receiver-native/WindowsSenderSession.cs)

Если надо менять desktop playback:
- [MediaFoundationD3D11PlaybackController.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/receiver-native/MediaFoundationD3D11PlaybackController.cs)
- [LibVlcPlaybackController.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/receiver-native/LibVlcPlaybackController.cs)

Если надо менять routing / LAN / codec negotiation:
- [Program.cs](c:/Users/VAALI/AndroidStudioProjects/EvertyGame/control-plane/Program.cs)

## 11. Текущие defaults

- Android receiver:
  - quick presets
  - low-latency bias

- Windows host:
  - sender preset `Game`
  - auto encoder selection
  - codec negotiation with AV1 preference

- Routing:
  - `prefer direct on LAN`
  - relay fallback

- Desktop client:
  - simple connect by code
  - advanced settings hidden behind advanced mode
