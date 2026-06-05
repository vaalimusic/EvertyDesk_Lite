# EvertyDesk Lite — гайд для Windows-разработчика

> Цель этого документа: дать разработчику на Windows всё необходимое, чтобы
> собрать проект, протестировать новый EVRT-стек живьём и понять архитектуру.
>
> EVRT-протокол и алгоритмы низкой задержки — разработка **Артура Валиева**
> (перенос из EvertyGame). См. шапки файлов `src/evrt*.rs`.

---

## 1. Что такое EVRT и зачем

EvertyDesk Lite — это форк-надстройка над транспортом RustDesk (hbbs/hbbr),
к которой прикручен собственный игровой стриминговый стек **EVRT** (Everty
Real-Time): прямой UDP, нулевая задержка очереди, адаптивная подстройка
битрейта по обратной связи от клиента.

```
RustDesk даёт:  rendezvous (hbbs), relay (hbbr), auth, NAT discovery
EVRT даёт:      прямой UDP-стрим, FrameReassembler, LatestAccessUnitQueue,
                FeedbackLoop, AdaptiveRelief, Windows perf hints
```

Соединение: TCP relay (RustDesk) для auth → хост сообщает свой EVRT UDP-порт
через `Misc{EvrtUdpPort}` → клиент пробивает NAT и переключается на прямой UDP.
TCP relay остаётся как fallback.

---

## 2. Требования к окружению

| Компонент | Версия | Зачем |
|-----------|--------|-------|
| Rust | stable 1.75+ | основной язык |
| MSVC Build Tools | 2019/2022 | линковка под Windows, C++ shim |
| Windows | 10 1803+ / 11 | Media Foundation, WASAPI loopback |
| (опц.) NVIDIA Video Codec SDK | 12.x / 13.x | NVENC + zero-copy |
| (опц.) NVIDIA GPU + драйвер | — | тест NVENC zero-copy |

Установка Rust под Windows:
```powershell
# через rustup, toolchain MSVC (не GNU!)
rustup default stable-x86_64-pc-windows-msvc
```

---

## 3. Сборка

### Базовая (Media Foundation H264/H265, без NVENC)
```powershell
cargo build --release
```
Фичи по умолчанию: `live-h264`, `live-vp9-mf`. Этого достаточно для теста
EVRT + аудио + pipeline на любой Windows-машине.

### С NVENC (нужен NV Codec SDK)
```powershell
$env:NV_CODEC_SDK = "C:\path\to\Video_Codec_SDK_13.0.37"
cargo build --release --features live-nvenc-sdk
```
`build.rs` найдёт SDK, скомпилирует `src/nvenc_shim.cpp` и включит
`nvenc_api_ffi`. Без SDK эта фича просто не активна (не ломает сборку).

### Запуск
```powershell
.\target\release\evertydesk-lite.exe
```

---

## 4. Что НУЖНО протестировать живьём (ничего не проверено на железе!)

Весь EVRT-стек написан и компилируется, но **ни разу не запускался на Windows**.
Это критично. Порядок проверки от простого к сложному:

### 4.1. TCP relay (базовый путь, должен работать как раньше)
- [ ] Запустить хост на машине A, клиент на машине B
- [ ] Подключиться по ID — должно пойти видео через relay (H264/H265)
- [ ] Проверить мышь/клавиатуру (это чинили — `recv_cipher` теперь
      расшифровывает ввод; раньше был баг)
- [ ] Проверить shell/терминал (чинили `dummy_tx` → вывод теперь доходит)

### 4.2. EVRT прямой UDP
- [ ] В логах хоста: `EVRT: UDP сокет открыт на порту NNNNN`
- [ ] В логах хоста: `EVRT: Misc{EvrtUdpPort=NNNNN} sent → клиент`
- [ ] В логах клиента: `[evrt-client] EvrtUdpPort=NNNNN получен`
- [ ] В логах клиента: `[evrt-client] прямой UDP → IP:порт`
- [ ] В логах хоста: `EVRT: punch от <client_addr>`
- [ ] UI-бейдж: должен показать `EVRT UDP прямой → host:port` (зелёный)
- [ ] Метрики EVRT в UI: pressure / arrival_delta / jitter / fps
- [ ] **Сравнить задержку** TCP relay vs EVRT (мышь→экран) — должна упасть

> Если EVRT не поднимается — клиент остаётся на TCP relay (graceful fallback).
> Смотреть: открыт ли UDP-порт в firewall, доходит ли punch.

### 4.3. Firewall
EVRT использует выделенный UDP-порт. Зафиксировать его в конфиге для
firewall-правила:
```jsonc
// config: evrt_udp_port (0 = случайный, рекомендуется зафиксировать)
"evrt_udp_port": 45123
```
```powershell
netsh advfirewall firewall add rule name="EvertyDesk-EVRT-UDP" `
  dir=in action=allow protocol=UDP localport=45123
```

### 4.4. Аудио (WASAPI loopback)
- [ ] На хосте играет звук → в логах `EVRT: WASAPI loopback старт`
- [ ] На клиенте `EVRT Audio: 48000Hz 2ch 16bit` + `WASAPI playback инициализирован`
- [ ] Звук слышен на клиенте, синхронен с видео
- [ ] При потере пакетов — нет фатальных крэшей (щелчки допустимы, jitter-буфер аудио ещё не сделан)

### 4.5. Статичные кадры (экономия трафика)
- [ ] Оставить статичный экран → в телеметрии pipeline `skipped_static` растёт
- [ ] Трафик при статике должен упасть почти до нуля

### 4.6. Баг зависания (0xcfffffff) — был исправлен
- [ ] Отключиться от хоста несколько раз подряд
- [ ] Приложение НЕ должно зависать на выходе (был timeout join 2с на MF encoder)

---

## 5. Архитектура: единый pipeline

Главное изменение — **один захват, один энкодер, два отправителя**:

```
┌──────────────┐   ┌──────────────┐
│ CaptureThread│──►│EncodeThread  │  (FrameChangeDetector: skip статики)
└──────────────┘   │ MultiEncoder │  MF→VideoToolbox→NVENC→OpenH264→PNG
                   └──────┬───────┘
                          │ EncodedFrame
                   ┌──────▼───────┐
                   │  Dispatcher  │
                   └──┬────────┬──┘
              ┌───────┘        └────────┐
       ┌──────▼──────┐         ┌────────▼────────┐
       │  TcpSender  │         │  EvrtUdpSender   │
       │ TcpItem::   │         │  EVRT packetize  │
       │ Video|Peer  │         │  + FeedbackLoop  │
       └─────────────┘         └─────────────────┘
```

Файлы:
| Файл | Ответственность |
|------|-----------------|
| `src/video_pipeline.rs` | единый пайплайн, dispatcher, TCP+EVRT senders, телеметрия |
| `src/evrt.rs` | EVRT протокол (пакеты, парсинг, SessionConfig, feedback) |
| `src/evrt_session.rs` | хост-сторона EVRT: UDP-доставка, perf hints, adaptive relief |
| `src/evrt_client.rs` | клиент-сторона: приём, reassembly, декод, feedback |
| `src/evrt_audio.rs` | WASAPI capture (хост) + playback (клиент) |
| `src/frame_queue.rs` | LatestAccessUnitQueue, ChannelReassembler, AdaptiveJitter |
| `src/fsr.rs` | AMD FSR 1.0 (EASU+RCAS) апскейл |
| `src/host.rs` | regs loop, relay auth, MultiEncoder, input injection |
| `src/nvenc_shim.cpp` | NVENC C++ shim (+ zero-copy `encode_texture`) |

---

## 6. NVENC zero-copy — НЕ ДОДЕЛАНО, требует Windows+NVIDIA

Примитив готов:
- `src/nvenc_shim.cpp` → `everty_nvenc_encode_texture()` — `CopyResource`
  (GPU→GPU) через shared handle вместо `UpdateSubresource` (CPU→GPU)
- `src/nvenc.rs` → `NvencEncoder::encode_texture()` — Rust-биндинг

Что осталось (нужно NVIDIA-железо для разработки/теста):
1. DXGI capture (`src/capture.rs`) должен создавать staging-текстуру с
   `D3D11_RESOURCE_MISC_SHARED` и отдавать `GetSharedHandle()`
2. Прокинуть shared handle через границу capture→encode потока
3. `MultiEncoder::encode()` вызывать `encode_texture` вместо `encode_bgra`
   когда хэндл доступен и активен NVENC
4. Учесть: устройства захвата и энкодера РАЗНЫЕ — shared handle это решает,
   но нужна синхронизация (keyed mutex) при кросс-девайс доступе

Выигрыш: устранение roundtrip GPU→CPU→GPU = меньше latency, меньше нагрузка.
Это главная оптимизация EvertyGame, которую тут пока недоиспользуем.

---

## 7. Известные проблемы / технический долг

| # | Проблема | Влияние | Где |
|---|----------|---------|-----|
| 1 | NVENC zero-copy не подключён end-to-end | latency не оптимальна на NVIDIA | shim+capture |
| 2 | Аудио без jitter-буфера | щелчки при потере пакетов | `evrt_audio.rs` |
| 3 | ROI всегда full-screen (w=0,h=0) | нет оптимизации грязных регионов | `evrt_session.rs` |
| 4 | Enhancement stream не реализован | только base layer | `evrt.rs` (есть константы) |
| 5 | 29 warnings (dead-code telemetry от старого video_loop) | косметика | `host.rs` |
| 6 | `transport::login_request_uses_32_byte_password_hash` тест падал ДО нас | не наша регрессия | `transport.rs` |
| 7 | Ничего не тестировано на Windows | **главный риск** | весь EVRT-стек |

---

## 8. Полезные команды

```powershell
cargo build --release                          # базовая сборка
cargo build --release --features live-nvenc-sdk # с NVENC
cargo test                                      # 61 тест (1 предсуществующий падёж)
cargo test evrt                                 # только EVRT-тесты
cargo check 2>&1 | Select-String "warning"      # посмотреть warnings
```

Логи пишутся в stderr с префиксами: `[evrt]`, `[evrt-client]`, `[pipeline]`,
`[host]`, `[evrt-audio]`. Запускать из консоли чтобы видеть.

---

## 9. С чего начать новому разработчику

1. Собрать `cargo build --release`, запустить, подключиться TCP relay — убедиться что база работает
2. Прочитать `ROADMAP.md` — там приоритеты и статус каждой фичи
3. Прочитать `src/evrt.rs` (протокол) → `src/video_pipeline.rs` (как всё связано)
4. Прогнать чек-лист из §4 на двух машинах
5. Самое ценное для проекта прямо сейчас: **подтвердить что EVRT UDP реально
   поднимается** (§4.2) — без этого весь стек работает только в теории
