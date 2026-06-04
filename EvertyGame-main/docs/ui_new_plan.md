# Everty UI Plan

Статус: 99%

## Уже сделано

- Desktop host переведен на product-style layout.
- Добавлен hero block со статусом и primary CTA.
- HUD и diagnostics спрятаны в drawer.
- Desktop client получил short code connect flow.
- Android receiver получил quick presets и advanced settings.
- Route split сделан: relay registration отдельно от transport.
- Auto encoder policy и codec negotiation уже внедрены.
- Auto encoder selection детерминирован:
  1. NVIDIA NVENC
  2. Intel Quick Sync
  3. Media Foundation
  4. Software
- Host scroll behavior упрощен, ручной wheel handler убран.
- Android AV1 Max preset доступен в Advanced или если device поддерживает AV1 decode.
- Android preset cards стали крупнее и читабельнее.
- Avalonia desktop shell создан и уже умеет restore managed session при старте.
- Restore startup обернут в try/catch.
- Advanced controls в Avalonia shell свернуты по умолчанию.
- Playback surface в shell теперь показывает реальные route/codec/endpoint facts.
- Shared control-plane contracts вынесены в отдельный project.
- Avalonia UI prefs теперь сохраняют `Advanced` и `Diagnostics` state между перезапусками.
- Host code теперь виден крупно и копируется из hero/host card.
- Client auto-selects host by saved short code after refresh.
- Host/client actions now switch shell to the relevant tab automatically.
- Hotkeys added for host code and diagnostics copy.
- Escape collapses advanced/diagnostics; Ctrl+Shift+1/2 switch tabs.

## Дальше

### 1. Android receiver UX

- Довести quick presets до финального состояния.
- Оставить manual fields только в Advanced.
- Сохранить preset selection между reconnect.
- Проверить, что `Connect by code` не сбрасывает preset.

### 2. Desktop host UX

- Уплотнить hero, profile и telemetry cards.
- Убрать лишний raw debug с main screen.
- Оставить diagnostics только в drawer.
- Довести scroll behavior до нормального состояния.

### 3. Encoder policy

- `Auto` должен выбирать:
  1. `NVIDIA NVENC`
  2. `Intel Quick Sync`
  3. `Media Foundation`
  4. `Software`
- Выбор должен быть до старта, а не после runtime fail.
- Отсутствие `nvcuda.dll` не должно ломать всю сессию.

### 4. Codec policy

- `AV1 -> HEVC -> AVC`.
- AV1 только если host encode и client decode оба поддерживают.
- Fallback должен быть явный.

### 5. LAN route

- Same LAN -> direct first.
- Relay -> fallback.
- Route state должен быть виден в HUD.

### 6. Avalonia desktop migration

- Довести client playback/connect flow до финала.
- Довести cross-platform target cleanup.
- Убрать последние WinForms leftovers.

## Files

- `receiver-native/MainForm.cs`
- `receiver-native/WindowsSenderSession.cs`
- `receiver-native/DesktopControlPlaneClient.cs`
- `control-plane/Program.cs`
- `app/src/main/java/com/everty/evertygame/receiver/PcReceiverModeScreen.kt`
- `app/src/main/java/com/everty/evertygame/receiver/PcReceiverClientController.kt`
- `desktop-avalonia/MainWindowViewModel.cs`
