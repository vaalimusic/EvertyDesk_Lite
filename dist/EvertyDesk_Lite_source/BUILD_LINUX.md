# EvertyDesk Lite — сборка на Linux / Astra

Лёгкий RustDesk-совместимый клиент. На Linux работает **подключение** и
экспериментальный хостинг через универсальный backend:
X11/XTest для Astra/RED OS/Ubuntu на Xorg, fallback через `grim`/`ydotool`
для Wayland-сессий, где это разрешено окружением.

## 1. Зависимости

```bash
# Rust (если ещё нет)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Системные библиотеки (Astra / Debian / Ubuntu)
sudo apt update
sudo apt install -y build-essential pkg-config cmake nasm \
    libx11-dev libxcb1-dev libxkbcommon-dev \
    libgl1-mesa-dev libegl1-mesa-dev \
    libasound2-dev libxtst-dev xdotool

# Опционально для Wayland-хоста:
sudo apt install -y ydotool grim wtype || true
```

## 2. Сборка

На Linux НЕ используй дефолтные фичи — `live-vp9-mf` тянет Windows-only код.

### Astra Linux → режим `system` (H264 + VP9 через системный libvpx)

```bash
sudo apt install libvpx-dev           # системный libvpx (1.7.x)
cd EvertyDesk_Lite
chmod +x scripts/build-linux.sh
./scripts/build-linux.sh system        # H264 + VP9 через -lvpx
```

Это линкуется к **готовому системному libvpx** (`libvpx-dev`), а не собирает
его из исходников. Сборка из исходников на Astra падает: libvpx содержит код
AVX-512 (`_mm512_reduce_add_epi32`), который понимает только GCC 7+, а на Astra
GCC старый. Системный libvpx собран мейнтейнерами под тулчейн Astra — работает.

Теперь клиент декодирует **и H264, и VP9** → живое видео с любого хоста.

### Только H264 (без VP9, минимальная сборка)

```bash
./scripts/build-linux.sh h264
```

### VP8/VP9 из исходников (системы с GCC ≥ 7, не Astra)

```bash
./scripts/build-linux.sh auto          # libvpx из исходников
```

Бинарь: `target/release/evertydesk-lite`

## 3. Запуск

```bash
# Обычный запуск (сам подберёт рендерер: wgpu → OpenGL → CPU)
./target/release/evertydesk-lite

# Если GLX/OpenGL ломается (Astra на SVGA3D: "GLXBadContextTag") —
# принудительный CPU-интерфейс без OpenGL (minifb):
EVERTYDESK_RENDERER=software ./target/release/evertydesk-lite
```

## 4. Переменные окружения

| Переменная | Значение | Что делает |
|------------|----------|-----------|
| `EVERTYDESK_RENDERER` | `software` | CPU-интерфейс (minifb), без OpenGL/Vulkan — **надёжно для VM/Astra** |
| `EVERTYDESK_RENDERER` | `glow` | OpenGL |
| `EVERTYDESK_RENDERER` | `wgpu` | Vulkan/wgpu |
| `EVERTYDESK_RENDERER` | `host` | headless-хост без GUI |

## Заметки

- `software`-режим (minifb) рисует на CPU и не трогает GLX — это решение для
  Astra, где `GLXBadContextTag` валит eframe/winit.
- Сервер по умолчанию `edesk.server1.everty.ru`. Меняется в настройках.
