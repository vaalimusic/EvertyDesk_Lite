# EvertyDesk Lite — сборка на Linux / Astra

Лёгкий RustDesk-совместимый клиент. На Linux работает **подключение** (исходящие
сессии) к любым RustDesk/EvertyDesk-хостам. Хостинг (приём входящих) с экрана
Linux пока только на Windows — захват X11 в планах.

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
    libasound2-dev
```

## 2. Сборка

На Linux НЕ используй дефолтные фичи — `live-vp9-mf` тянет Windows-only код.

### Astra Linux → собирай в режиме `h264`

```bash
cd EvertyDesk_Lite
chmod +x scripts/build-linux.sh
./scripts/build-linux.sh h264
```

**Почему не VP9 на Astra:** libvpx содержит код AVX-512
(`_mm512_reduce_add_epi32`), который понимает только GCC 7+. На Astra
GCC старый → сборка libvpx падает на линковке. VP9 на Astra недоступен,
пока не обновишь GCC (`sudo apt install gcc-8 g++-8` + `update-alternatives`).

H264-only клиент сообщает хосту «шлю/принимаю только H264», и хост шлёт H264
(у RustDesk-хостов есть программный H264-энкодер). Этого достаточно.

### VP8/VP9 (только на системах с GCC ≥ 7)

```bash
./scripts/build-linux.sh auto          # H264 + VP8/VP9 (libvpx из исходников)
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
