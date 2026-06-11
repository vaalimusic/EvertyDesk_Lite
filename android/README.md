# EvertyDesk Android — клиент (исходящие подключения)

> Только клиент: подключиться к хосту по ID+паролю, смотреть экран, управлять
> тачем. Хост-режим (раздача своего экрана) — не входит в эту версию.

## Архитектура

```
┌─────────── Android (Kotlin) ───────────┐
│ MainActivity  — экран подключения        │
│ AddressBookApi — login + /api/ab/*        │
│ RemoteView    — отрисовка кадров + тачи  │
│ NativeClient  — JNI обёртка               │
│        ↕ JNI (libevertydesk_core.so)     │
├─────────── Rust ядро (наш код) ─────────┤
│ android_ffi.rs — мост                    │
│ transport + evrt_client + декод H264     │
└──────────────────────────────────────────┘
```

Весь клиент (подключение к hbbs, relay, auth, приём H264, **декод через
OpenH264**, EVRT) — это наш Rust. Kotlin только UI + блит RGBA + тачи.

## Что нужно установить (один раз)

1. **Rust Android targets:**
   ```bash
   rustup target add aarch64-linux-android armv7-linux-androideabi
   ```
2. **Android NDK** (через Android Studio → SDK Manager → NDK, или standalone)
3. **cargo-ndk:**
   ```bash
   cargo install cargo-ndk
   ```
4. **Android Studio** (для сборки APK и эмулятора/устройства)

## Сборка нативной библиотеки (.so)

Из корня проекта (`EvertyDesk_Lite/`):

```bash
# arm64 (современные телефоны) + armv7 (старые)
cargo ndk \
  -t arm64-v8a -t armeabi-v7a \
  -o android/app/src/main/jniLibs \
  build --release \
  --lib \
  --no-default-features \
  --features "live-h264,android-client"
```

Это соберёт `libevertydesk_core.so` под каждую архитектуру и положит в
`android/app/src/main/jniLibs/<abi>/`.

> ⚠️ `--no-default-features` отключает Windows MF / NVENC (их нет на Android).
> `live-h264` даёт OpenH264-декод. `android-client` включает JNI-мост.

> OpenH264 на Android собирается из исходников через NDK-компилятор — это
> часть `openh264` крейта (feature `live-h264` → source build). Если будут
> проблемы со сборкой openh264 под NDK, см. раздел «Известные нюансы» ниже.

## Сборка APK

```bash
cd android
./gradlew assembleDebug      # APK в app/build/outputs/apk/debug/
# или открой папку android/ в Android Studio и нажми Run
```

## Запуск

1. Подключи телефон (USB-отладка) или эмулятор
2. `./gradlew installDebug` или Run в Android Studio
3. Введи ID хоста + пароль → Подключиться
4. Во вкладке «Контакты» можно войти в `desk.everty.ru`, синхронизировать
   адресную книгу и подключаться к устройствам из списка.

## Поток данных

| Шаг | Где |
|-----|-----|
| Ввод ID/пароль | `MainActivity.connect()` |
| API/login + адресная книга | `AddressBookApi` + `MainActivity.showContactsScreen()` |
| Настройки API/ID/relay/public key | `MainActivity.showSettingsScreen()` → `SharedPreferences`; встроенный профиль в UI маскируется |
| `nativeStart` → запуск сессии | `NativeClient.start()` → `android_ffi.rs` → `TransportClient::run_session` |
| Приём H264 + декод | Rust `transport` + `decode_frame_loop` (OpenH264) |
| RGBA кадр → `latest` | `android_ffi.rs::collect_events` |
| `nativePollFrame` → Bitmap | `RemoteView.pullFrame` (16мс опрос) |
| Тач → mouse | `RemoteView.onTouchEvent` → `nativeTouch` → `SessionCommand::Mouse*` |

## Что дальше (план развития)

- [ ] **MediaCodec** аппаратный H264-декод (сейчас OpenH264 софт — медленнее)
- [ ] **SurfaceView** вместо View+Bitmap (zero-copy отрисовка)
- [x] Базовая адресная книга через `desk.everty.ru`
- [x] Настройки API / ID server / relay / public key
- [x] Сохранение недавнего ID
- [ ] Жесты: правый клик (long-press), скролл (два пальца), клавиатура
- [ ] EVRT прямой UDP на Android (сейчас работает TCP relay)
- [ ] Аудио воспроизведение (AudioTrack)

## Известные нюансы

- **OpenH264 под NDK**: крейт `openh264` с `live-h264` собирает libopenh264 из
  исходников. Нужен рабочий NDK-toolchain (cargo-ndk настраивает CC/AR). Если
  не соберётся — альтернатива: декод через Android MediaCodec в Kotlin (тогда
  Rust отдаёт сырые H264 access units, а не RGBA — будущая доработка).
- **minSdk 24**: для современного рантайма. Можно понизить если нужно.
- Конфиг подключения на Android задаётся во вкладке «Настройки» и передаётся
  в `nativeStart`. Встроенный профиль EvertyDesk в UI показывается как
  `********`; свои серверы можно включить только полным набором:
  API URL, ID server, relay server и public key.

---

EVRT-протокол и ядро — разработка Артура Валиева. См. шапки `src/evrt*.rs`.
