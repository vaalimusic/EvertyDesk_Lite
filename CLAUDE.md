# EvertyDesk Lite — контекст для Claude Code

---

## ⚡ ПРАВИЛО №1 — EVRTCK-Silicon

**Когда обнаружено совместимое железо — EVRTCK больше не тайловый программный кодер.**

EVRTCK переключается в режим **EVRTCK-Silicon** (EVRTCK-SCWMCOC —
*Silicon Compatible With Modern Circuits Of Codec*) и становится
полноценной альтернативой H264 / H265 / AV1.

### Что это означает на практике

| Режим | Когда | Бэкенд кодирования |
|-------|-------|-------------------|
| **EVRTCK-SW** | Silicon не обнаружен | XOR-diff тайлы, zstd/ZRLE, чистый Rust |
| **EVRTCK-Silicon** | HW encoder найден | Silicon encoder + EVRT протокол |

В режиме Silicon:
- Тайловый XOR-diff **не используется**
- Кодирование делегируется **кремнию** (GPU/NPU/DSP)
- EVRT транспортный слой остаётся прежним — FeedbackLoop, LatestAccessUnitQueue, адаптивная буферизация
- Результат конкурирует с H264/H265/AV1 по качеству при **значительно меньшей задержке** за счёт EVRT

### Матрица совместимого кремния

| Платформа | Silicon | API |
|-----------|---------|-----|
| NVIDIA (Win/Linux) | NVENC | `src/nvenc.rs` |
| Windows любой GPU | Media Foundation | `src/mf_encode.rs` |
| Android (Qualcomm / MediaTek / Unisoc HW) | MediaCodec HW encoder | Android NDK `AMediaCodec` |
| Apple (M-series, A-series, Neural Engine) | VideoToolbox | `src/videotoolbox.rs` |
| AMD / Intel (планируется) | AMF / QSV | — |

### Правило детекции

```
if silicon_encoder_available(platform) {
    mode = EVRTCK_SILICON   // кремний кодирует, EVRT транспортирует
} else {
    mode = EVRTCK_SW        // XOR-diff тайлы, CPU only
}
```

Детекция происходит **один раз при старте сессии**. Результат кэшируется.
Если silicon выдаёт ошибку в процессе — мгновенный fallback на EVRTCK-SW
без разрыва сессии.

### Почему EVRTCK-Silicon лучше чем просто H264 по EVRT

H264/H265/AV1 по EVRT уже работают (см. `evrt_session.rs`).
EVRTCK-Silicon отличается тем, что:
1. **Протокол единый** — клиент не знает какой silicon на хосте
2. **Адаптация в реальном времени** — FeedbackLoop управляет битрейтом silicon encoder
3. **Переключение SW↔Silicon прозрачно** — клиент получает те же EVRT-пакеты
4. **ROI (Region of Interest)** — EVRT знает какие регионы экрана изменились,
   silicon encoder получает hint → меньше waste на статичные зоны

---

## Что это

EvertyDesk Lite — нативное приложение для удалённого доступа, написанное на Rust.
Собственная разработка Артура Валиева. **Не форк RustDesk** — хотя содержит
совместимый с RustDesk транспортный слой (rendezvous/relay) для подключения к
существующей инфраструктуре.

Продукт состоит из двух самостоятельных клиентов, живущих в одном репозитории:

| Клиент | Путь | Стек |
|--------|------|------|
| Desktop (Windows/Linux) | `src/` + бинарь | Rust + egui/eframe |
| Android | `android/` + `src/` как .so | Kotlin + JNI → Rust cdylib |

## Репозитории экосистемы

| Репо | Путь | Назначение |
|------|------|-----------|
| **EvertyDesk Lite** | `D:\github_project\EvertyDesk_Lite` | Этот репо |
| **API сервер** | `D:\github_project\rustdesk-api-server-pro-master` | Go бэкенд + Vue 3 админка |
| **Workflows** | `D:\github_project\everty-workflows-clean` | CI-сборки RustDesk-based клиентов (другой продукт) |

**Важно:** EvertyDesk Lite и everty-workflows — это два разных продукта.
Lite собирается локально (`cargo build` / `gradlew assembleDebug`), не через GitHub CI.

---

## Архитектура

```
┌─────────────────────────────────────────────────────────────┐
│                    EvertyDesk Lite                          │
│                                                             │
│  ┌─────────────────────┐   ┌─────────────────────────────┐ │
│  │  Desktop (Win/Linux) │   │  Android                    │ │
│  │                     │   │                             │ │
│  │  src/main.rs        │   │  android/app/.../*.kt       │ │
│  │  egui/eframe UI     │   │  MainActivity, RemoteView   │ │
│  │  Software renderer  │   │  VideoDecoder (HW decode)   │ │
│  │                     │   │  PerfStats                  │ │
│  └──────────┬──────────┘   └──────────┬──────────────────┘ │
│             │                         │ JNI (.so)           │
│             └──────────┬──────────────┘                     │
│                        ▼                                    │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  Rust Core (src/lib.rs)                             │   │
│  │                                                     │   │
│  │  transport.rs    — RustDesk rendezvous/relay        │   │
│  │  evrt_client.rs  — EVRT UDP клиент                 │   │
│  │  evrt_session.rs — EVRT UDP хост/сессия            │   │
│  │  evrt.rs         — EVRT протокол (бинарный формат) │   │
│  │  evrtck.rs       — EVRTCK кодек (тайловый лоссл.)  │   │
│  │  host.rs         — захват экрана + кодирование     │   │
│  │  video.rs        — видео-декодер (SW, desktop)     │   │
│  │  android_ffi.rs  — JNI_OnLoad + class caching      │   │
│  │  android_video.rs — JNI-мост → VideoDecoder.kt     │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

---

## Протоколы и кодеки

### EVRT (EvertyDesk Remote Transport)

UDP-протокол с заголовком 24 байта, MTU-safe (макс. пакет 1200 байт).
Разработан Артуром Валиевым, изначально для EvertyGame (C#), портирован в Rust.

```
Magic(4) | Version(1) | Type(1) | Flags(2) | FrameId(4) |
PacketIndex(2) | PacketCount(2) | PresentationTimeUs(8) | Payload...
```

**Типы пакетов:** SESSION_CONFIG, CODEC_CONFIG, VIDEO_FRAME, AUDIO_FRAME,
INPUT_EVENT, KEEPALIVE, CODEC_NAME (ASCII строка при смене кодека).

**Codec switch flow (критично):** хост всегда стартует с H265-сессии,
немедленно шлёт CODEC_CONFIG "H264" и переключается. На Android CODEC_CONFIG
и первый IDR приходят близко по времени через UDP — возможен reorder.
Клиент всегда рекламирует `h264=true h265=false av1=false`.

- `src/evrt.rs` — формат пакетов, сборка/разборка
- `src/evrt_client.rs` — клиентский приём и декод
- `src/evrt_session.rs` — хостовая сессия, захват и отправка

### EVRTCK (EvertyDesk Remote Transport Codec)

Собственный лосслесс тайловый кодек. Тайл 32×32px.
- Статичный тайл: 1 бит (dirty-map)
- Solid-color тайл: 5 байт
- Delta тайл: XOR от предыдущего кадра → ZRLE или zstd level-1

Используется в "support mode" (не игровой режим). Нет аппаратного ускорения —
чистый Rust, работает везде. FPGA-симуляция — `fpga/`.

- `src/evrtck.rs` — энкодер/декодер
- `src/evrtck_wgpu.rs` — GPU backend через WGPU (feature: `gpu-accel`)

### RustDesk-совместимый транспорт

- `src/transport.rs` — rendezvous (порт 21116), relay (21117), RustDesk protobuf
- `src/rustdesk_proto.rs` — protobuf типы (LoginRequest, VideoFrame, MouseEvent и др.)
- `src/host.rs` — хост-режим: захват экрана, кодирование, relay-сессия

### RDP (через IronRDP)

Для подключения к ВМ через RDP — Hyper-V, VirtualBox VRDE, Proxmox, VMware.
- `ironrdp-viewer/` — standalone RDP viewer
- `vendor/ironrdp-bulk/` — форк с фиксом HistoryBufferOverflow (VirtualBox VRDE)
- `vendor/ironrdp-session/` — форк с фиксом out-of-bounds в bitmap-apply

---

## Видеостек

### Desktop (Windows)

| Бэкенд | Файл | Зависимость |
|--------|------|-------------|
| H264 SW | `src/video.rs` | openh264 (feature: `live-h264`) |
| VP9 MF | `src/vp9_mf.rs` | Windows Media Foundation (feature: `live-vp9-mf`) |
| VP9 libvpx | `src/vpx_system.rs` | system libvpx (feature: `live-vpx-system`) |
| NVENC | `src/nvenc.rs` | NVIDIA SDK 13 (feature: `live-nvenc-sdk`) |
| MF Encode | `src/mf_encode.rs` | WinRT H264/H265 encoder |
| MF Video | `src/mf_video.rs` | MF декодер |

### Android (Hardware MediaCodec)

- `android/app/src/main/java/ru/everty/desklite/VideoDecoder.kt` — HW декодер
- Async MediaCodec API, Surface output (TextureView → GPU render, 0-copy)
- `android/app/src/main/java/ru/everty/desklite/PerfStats.kt` — счётчики
- `src/android_video.rs` — JNI мост к VideoDecoder.decodeFrame()
- `src/android_ffi.rs` — JNI_OnLoad, кэш GlobalRef для VideoDecoder и PerfStats

**Известные проблемы Android (решены):**
- Unisoc T7250: нет H265/AV1 HW декодера → `isMimeSupported()` отфильтровывает до попытки create
- Surface занята предыдущим декодером → release old decoders before creating new
- Failed configure() блокировал Surface → `c?.release()` в catch-блоке

---

## Бэкенды подключения к ВМ

`src/session_backend.rs` — `SessionBackend` трейт: `probe()` + `open()` + `close()`

| Бэкенд | Файл |
|--------|------|
| Proxmox (QEMU) | `src/proxmox_provider.rs` |
| VMware | `src/vmware_provider.rs` |
| libvirt | `src/libvirt_provider.rs` |
| VirtualBox VRDE | `src/virtualbox.rs`, `src/vbox_rdp.rs` |
| Hyper-V (WMI) | `src/hyperv.rs`, `src/hyperv_rdp.rs` |
| Smart Connect | `src/smart_connect.rs` — автовыбор бэкенда |
| Provider API | `src/provider_api.rs` — единый интерфейс |

---

## Сборка

### Desktop

```bash
# Windows (debug)
cargo build

# Windows (release)
cargo build --release

# Linux
cargo build --features live-vpx-system --no-default-features --features evrt-wasapi
# или через скрипт:
./scripts/build-linux.sh
```

**Зависимости Windows:** MSVC, Windows SDK, NVIDIA SDK 13 в `Video_Codec_SDK_13.0.37/`.
**Зависимости Linux:** libvpx-dev, libasound2-dev, libx11-dev.

### Android

```bash
cd android
./gradlew assembleDebug      # debug APK
./gradlew assembleRelease    # release APK (требует keystore)
```

Rust компилируется как часть Gradle-сборки через `cargo-ndk`.
NDK target: `aarch64-linux-android` (arm64-v8a).
Keystore: `android/keystore/evertydesk-lite-release.jks`, пароли в `android/keystore.properties`.

Никакого GitHub CI для Android — только локальная сборка.

---

## Структура папок

```
src/                    Rust ядро (и desktop main)
android/                Android-проект (Kotlin + Gradle)
  app/src/main/
    java/ru/everty/desklite/  Kotlin классы
    jniLibs/            Скомпилированные .so (генерируются Gradle)
    res/                Android ресурсы
docs/                   Техническая документация
scripts/                Build/install скрипты
fpga/                   EVRTCK HLS симуляция (Xilinx Vitis HLS)
vendor/                 Форки ironrdp-bulk и ironrdp-session
ironrdp-viewer/         Standalone RDP viewer
EvertyGame_other_project/  Отдельный C#/Android ресивер (другой проект, не Lite)
rustdesk-master/        Оригинальный RustDesk (справочный, не используется в сборке)
dist/                   Готовые релизы
benches/                Criterion бенчмарки (EVRTCK)
migrations/             SQL схема (если используется БД)
```

---

## Несовместимости и известные грабли

### 1. Android JNI — хрупкий class caching

`JNI_OnLoad` (`android_ffi.rs`) кэширует GlobalRef на `VideoDecoder` и `PerfStats`.
Если класс не найден при старте — все JNI-вызовы из фоновых Rust-потоков упадут.
`env.find_class()` из фонового потока использует системный ClassLoader, который
не видит APK-классы.

### 2. EVRT codec reorder (UDP)

CODEC_CONFIG "H264" и IDR-кадр могут прийти в обратном порядке через UDP.
Если IDR обработан до CODEC_CONFIG → создаётся H265 декодер → блокирует Surface.
**Решение:** `isMimeSupported()` + release-before-create в `VideoDecoder.kt`.

### 3. Два протокола, одна инфраструктура

Игровой режим (EVRT UDP) и обычный режим (RustDesk relay/TCP) используют
разные кодеки и транспорт, но один и тот же ID/relay сервер.
В Android: `isGameMode = useHardware`, `useHardware = (gameCodec != "EVRTCK")`.

### 4. IronRDP форки (vendor/)

`ironrdp-bulk` и `ironrdp-session` — форки с патчами специфичными для VirtualBox VRDE.
Без этих патчей RDP-сессии к VirtualBox периодически десинхронизируются или
падают с out-of-bounds паникой.

### 5. NVIDIA SDK header-only

`Video_Codec_SDK_13.0.37/` лежит в репо — только заголовки (.h).
`nvenc.rs` дёргает драйвер динамически через `libnvenc.so` / `nvEncodeAPI64.dll`.
Если SDK нет на машине — всё равно компилируется, просто NVENC-путь недоступен.

### 6. FSR / GPU-accel — экспериментальные фичи

`fsr-gpu` и `gpu-accel` (WGPU) — отключены по умолчанию. Не использовать
в проде без явного тестирования на целевом железе.

---

## Ключевые файлы

| Файл | Роль |
|------|------|
| `src/evrt.rs` | Бинарный протокол EVRT — формат пакетов |
| `src/evrt_client.rs` | Клиентский приём UDP, codec switch loop |
| `src/evrt_session.rs` | Хостовая EVRT-сессия, захват, отправка |
| `src/evrtck.rs` | EVRTCK кодек — лосслесс тайловый |
| `src/host.rs` | Хост-режим: rendezvous + relay-сессия |
| `src/transport.rs` | RustDesk rendezvous/relay клиент |
| `src/android_ffi.rs` | JNI_OnLoad, GlobalRef кэш |
| `src/android_video.rs` | JNI мост к VideoDecoder |
| `android/app/src/main/java/ru/everty/desklite/VideoDecoder.kt` | HW MediaCodec декодер |
| `android/app/src/main/java/ru/everty/desklite/MainActivity.kt` | Android UI, TextureView |
| `Cargo.toml` | Features: desktop-gui, android-client, live-h264, gpu-accel и др. |
| `android/app/build.gradle.kts` | Android сборка + cargo-ndk конфиг |
