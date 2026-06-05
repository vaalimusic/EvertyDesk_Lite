# EVRT Roadmap — игровой стриминговый стек

> **EVRT** (Everty Real-Time) — протокол и алгоритмы низкой задержки разработки
> **Артура Валиева** (Artur Valiev), перенос из многолетней работы над EvertyGame.
> Атрибуция в шапках `src/evrt*.rs`, `src/frame_queue.rs`.
>
> Общий план проекта: см. `ROADMAP.md`. Этот документ — про EVRT-надстройку.
> Гайд для Windows-разработчика: см. `WINDOWS_DEV_GUIDE.md`.

Статусы: `✅ готово` · `🟡 частично` · `🔬 не протестировано на железе` · `❌ не начато` · `⏸ отложено`

---

## TL;DR текущего состояния

Весь EVRT-стек **написан и компилируется**, 61 unit-тест зелёный, полный билд
проходит. **Но end-to-end на Windows ни разу не запускался** — это главный риск
и следующий обязательный шаг.

```
RustDesk транспорт (hbbs/hbbr, auth, NAT)  ──┐
                                              ├──► EvertyDesk Lite
EVRT стек (прямой UDP, low-latency, адаптация)┘
```

---

## Фаза 1 — Ядро EVRT  ✅ написано / 🔬 не протестировано

| Компонент | Статус | Файл |
|-----------|--------|------|
| EVRT протокол (24б заголовок, MTU 1200, типы пакетов) | ✅ | `evrt.rs` |
| FrameReassembler (UDP → кадры, waitForKeyframe) | ✅ | `frame_queue.rs` |
| LatestAccessUnitQueue (prefer_latest, hard_reset) | ✅ | `frame_queue.rs` |
| AdaptiveJitter (динамический буфер по pressure) | ✅ | `frame_queue.rs` |
| AdaptiveRelief (хост ↓ битрейт, 3 шага, гистерезис) | ✅ | `frame_queue.rs` |
| FeedbackLoop (клиент → хост: pressure, deltas) | ✅ | `evrt_client.rs` |
| Windows perf hints (timeBeginPeriod(1), High prio) | ✅🔬 | `evrt_session.rs` |
| Точный тайминг (nextFrameDueTicks, spin <1.5мс) | ✅ | `video_pipeline.rs` |
| HNS PTS монотонный | ✅ | `video_pipeline.rs` |
| **EVRT поднимается end-to-end** | 🔬 | ← главный непроверенный пункт |

---

## Фаза 2 — Интеграция с RustDesk  ✅ написано / 🔬

| Компонент | Статус | Заметки |
|-----------|--------|---------|
| Misc{EvrtUdpPort} обмен (tag 100) | ✅🔬 | хост → клиент |
| Выделенный EVRT UDP сокет | ✅ | не конкурирует с hbbs-сокетом |
| Один запрос к hbbs (+fallback на force_relay) | ✅ | было два запроса |
| Правильный IP хоста (PunchHoleResponse.socket_addr) | ✅ | было: слал на hbbs IP |
| Punch-hole координация | 🔬 | |
| TCP relay graceful fallback | ✅ | если EVRT не встал |

---

## Фаза 3 — Единый pipeline  ✅

| Компонент | Статус | Заметки |
|-----------|--------|---------|
| Один capture + один encoder | ✅ | было 2 параллельных → крэш MF |
| MultiEncoder каскад | ✅ | MF→VideoToolbox→NVENC→OpenH264→PNG |
| Dispatcher (TCP + EVRT) | ✅ | EVRT primary, IDR→TCP для sync |
| TcpItem (Video + Peer/shell) | ✅ | shell output больше не теряется |
| FrameChangeDetector (skip статики) | ✅ | экономия трафика и CPU |
| PipelineTelemetry (10с интервал) | ✅ | capture/encode/send ms, kbps |
| Удалён мёртвый video_loop (~470 строк) | ✅ | |

---

## Фаза 4 — Качество и фичи

| Компонент | Статус | Приоритет |
|-----------|--------|-----------|
| FSR 1.0 EASU+RCAS | ✅🔬 | — |
| FSR zero-copy (убран per-frame .to_owned) | ✅ | — |
| Аудио WASAPI (capture хост + playback клиент) | ✅🔬 | высокий |
| Аудио jitter-буфер (убрать щелчки) | ❌ | высокий |
| ROI метаданные | 🟡 | средний |
| ROI детекция грязных регионов | ❌ | средний |
| Enhancement stream (base+quality layer) | ❌ | низкий |

---

## Фаза 5 — Производительность (нужен Windows+NVIDIA)

| Компонент | Статус | Приоритет |
|-----------|--------|-----------|
| NVENC C++ shim (BGRA path) | ✅ | — |
| NVENC zero-copy примитив (encode_texture) | ✅ | высокий |
| **NVENC zero-copy end-to-end** | ❌ | высокий |
| Capture shared D3D11 texture (GetSharedHandle) | ❌ | высокий |
| HW-accel FSR (D3D11 compute shaders) | 🟡 | низкий |

### NVENC zero-copy — что осталось доделать
1. `capture.rs`: staging-текстура с `D3D11_RESOURCE_MISC_SHARED` + `GetSharedHandle()`
2. Прокинуть shared handle через границу capture→encode потока
3. `MultiEncoder::encode()` → вызывать `encode_texture()` вместо `encode_bgra()`
   когда handle доступен и NVENC активен
4. Keyed mutex для синхронизации (устройства захвата и энкодера разные)

Выигрыш: убрать roundtrip GPU→CPU→GPU. Главная latency-оптимизация EvertyGame.

---

## Фаза 6 — Свой signaling  ⏸ опционально

| Компонент | Статус | Заметки |
|-----------|--------|---------|
| relay-node (готов в EvertyGame C#) | ⏸ | можно задеплоить |
| Порт relay-node в Rust | ❌ | уход от hbbs, полный контроль |

---

## Критический путь

```
1. 🔬 Windows-сборка → подтвердить TCP relay (мышь/клавиатура/shell — фиксы)
2. 🔬 Подтвердить EVRT UDP реально поднимается  ← без этого стек только в теории
3. 🔬 Измерить latency TCP relay vs EVRT
4. 🔬 Проверить аудио end-to-end
5. ❌ Аудио jitter-буфер
6. ❌ NVENC zero-copy end-to-end (нужен NVIDIA-бокс)
7. ❌ ROI детекция грязных регионов
8. ⏸ Свой relay-node
```

---

## Технический долг

| # | Что | Серьёзность |
|---|-----|-------------|
| 1 | 29 warnings (orphaned telemetry от video_loop) | низкая |
| 2 | `login_request_uses_32_byte_password_hash` падал ДО нас | низкая (не регрессия) |
| 3 | Аудио без jitter-буфера → щелчки | средняя |
| 4 | NVENC zero-copy примитив не подключён | средняя |
| 5 | Нет сетевых интеграционных тестов (только unit) | средняя |

---

## Метрики «выстрелило»

На двух реальных Windows-машинах:
- [ ] EVRT UDP поднимается стабильно (не каждый раз TCP fallback)
- [ ] Задержка мышь→экран по EVRT заметно ниже TCP relay
- [ ] Аудио синхронно с видео
- [ ] Адаптация работает: плохая сеть → битрейт падает, картинка не замирает
- [ ] Стабильно при отключении/переподключении (нет 0xcfffffff)

---

## Сделано в этой сессии

Порт EVRT из EvertyGame · FrameReassembler · LatestAccessUnitQueue ·
AdaptiveJitter · AdaptiveRelief · FeedbackLoop · Windows perf hints ·
FSR 1.0 (+zero-copy срез) · Misc{EvrtUdpPort} · выделенный сокет ·
единый pipeline · MultiEncoder · dispatcher · FrameChangeDetector ·
PipelineTelemetry · фиксы (cipher/shell/зависание/двойной hbbs/IP) ·
NVENC zero-copy примитив · атрибуция авторства.
