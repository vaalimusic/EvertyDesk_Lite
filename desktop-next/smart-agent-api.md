# EvertyDesk Smart Agent — API Reference

Документация для разработчиков форка RustDesk, использующего встроенный Smart Agent (`agent_service.dart`).

---

## Содержание

1. [Обзор](#1-обзор)
2. [Аутентификация](#2-аутентификация)
3. [Идентификаторы машины](#3-идентификаторы-машины)
4. [Rate Limiting](#4-rate-limiting)
5. [Эндпоинты](#5-эндпоинты)
   - [POST /agent/heartbeat](#51-post-adminagentheartbeat)
   - [GET /agent/inbox](#52-get-adminagentinbox)
   - [POST /agent/notification/{id}/ack](#53-post-adminagentnotificationidack)
   - [POST /agent/notification/{id}/vote](#54-post-adminagentnotificationidvote)
   - [GET /agent/operators](#55-get-adminagentoperators)
   - [POST /agent/support-request](#56-post-adminagentsupport-request)
   - [POST /agent/support-request/respond](#57-post-adminagentsupport-requestrespond)
6. [Типы уведомлений](#6-типы-уведомлений)
7. [Жизненный цикл запроса помощи](#7-жизненный-цикл-запроса-помощи)
8. [Коды ошибок](#8-коды-ошибок)
9. [Интервалы опроса и backoff](#9-интервалы-опроса-и-backoff)
10. [Поле `options` в support_ping](#10-поле-options-в-support_ping)
11. [Конфигурация Dart-агента](#11-конфигурация-dart-агента)

---

## 1. Обзор

Smart Agent — это Dart-модуль (`agent_service.dart`), инжектируемый в форк RustDesk при сборке. Он работает внутри процесса RustDesk и обеспечивает:

- **Heartbeat** — регистрация машины и привязка к тенанту
- **Push-уведомления** — баннеры, опросы, обновление конфигурации
- **Запросы помощи** — двусторонний диалог пользователь ↔ оператор

Все эндпоинты агента **публичные** (без Authorization header). Идентификация тенанта осуществляется через `service_key` (slug компании), идентификация машины — через `machine_id`.

**Base URL:**
```
{api_server}/admin/agent/
```

Пример: `https://desk.everty.ru/admin/agent/heartbeat`

---

## 2. Аутентификация

Эндпоинты агента не требуют токена авторизации. Запросы проходят через публичный `adminParty` (без `AdminAuth` middleware).

Идентификация выполняется по двум полям:

| Поле | Тип | Описание |
|------|-----|----------|
| `service_key` | string | Slug сервисного аккаунта (тенанта). Передаётся в body (POST) или query param (GET). |
| `machine_id` | string | Уникальный 32-символьный hex-идентификатор машины. Генерируется и хранится на клиенте локально. |

Если `service_key` не совпадает ни с одним slug в базе — запрос принимается, но `service_account_id` будет `0` (машина не привязана к тенанту).

---

## 3. Идентификаторы машины

Агент использует **два разных идентификатора**:

### machine_id
- Генерируется агентом при первом запуске: 32 случайных символа (`[a-z0-9]`)
- Хранится в файле `{app_support_dir}/agent_id`
- Не совпадает с RustDesk numeric ID
- Используется во всех запросах агента как основной идентификатор

### rustdesk_id
- Числовой ID, назначаемый relay-сервером RustDesk (например, `123456789`)
- Читается из `.toml` конфига через `bind.mainGetMyId()` (нативный биндинг), с fallback на парсинг файла
- Передаётся в heartbeat как `rustdesk_id`
- Используется для авто-подключения оператора при принятии запроса помощи
- Может быть пустым, если relay ещё не назначил ID

**Путь к конфигу RustDesk по платформам:**

| Платформа | Путь |
|-----------|------|
| Windows | `%APPDATA%\{APP_NAME}\*.toml` |
| Linux | `~/.config/{APP_NAME}/*.toml` |
| macOS | `~/Library/Preferences/{APP_NAME}/*.toml` |

`APP_NAME` патчится в `hbb_common` при сборке (например, `"Everty Desk"`).

---

## 4. Rate Limiting

### Heartbeat (IP-based)

Сервер применяет sliding-window rate limit по IP-адресу источника:

- Окно: **1 минута**
- Лимит: **60 запросов в минуту** с одного IP (рассчитан на 60 машин за одним NAT)
- При превышении: `HTTP 429`

```json
{
  "error": "rate_limit",
  "retry_after": 60
}
```

### Support Request (machine-based)

- Максимум **5 запросов в час** с одной машины
- Максимум **2 открытых (нерешённых) запроса** одновременно
- Cooldown **30 минут** после отклонения запроса тем же оператором

При превышении: `HTTP 429`

```json
{
  "error": "rate_limit",
  "message": "Слишком много запросов. Подождите немного.",
  "retry_after_min": 60
}
```

---

## 5. Эндпоинты

### 5.1 POST /admin/agent/heartbeat

Регистрирует машину или обновляет время последней активности. Вызывается агентом каждые **1 минуту**.

Дополнительно выполняет автоматическую привязку RustDesk-устройства к тенанту: если `service_key` указан и для данного `rustdesk_id`/`hostname` найдено устройство в default-аккаунте — оно переносится в нужный тенант.

#### Request

```
POST /admin/agent/heartbeat
Content-Type: application/json
```

```json
{
  "machine_id":  "a3f8c1d2e4b5...",
  "service_key": "my-company",
  "hostname":    "DESKTOP-ABC123",
  "os":          "windows",
  "os_version":  "Windows 11 Pro 23H2",
  "rustdesk_id": "123456789"
}
```

| Поле | Тип | Обязательное | Описание |
|------|-----|:---:|---------|
| `machine_id` | string | ✓ | Уникальный ID агента на этой машине |
| `service_key` | string | — | Slug тенанта. Пустая строка = машина не привязана |
| `hostname` | string | — | Имя хоста (`Platform.localHostname`) |
| `os` | string | — | `"windows"`, `"linux"`, `"macos"`, `"android"` |
| `os_version` | string | — | Версия ОС (`Platform.operatingSystemVersion`) |
| `rustdesk_id` | string | — | Числовой ID RustDesk. Пустая строка если ещё не известен |

#### Response

**200 OK**
```json
{ "ok": true }
```

**400 Bad Request** — `machine_id` не передан
```json
{ "error": "machine_id required" }
```

**429 Too Many Requests** — IP rate limit
```json
{ "error": "rate_limit", "retry_after": 60 }
```

#### Поведение сервера

1. Если машина с таким `machine_id` уже существует — обновляет `hostname`, `os`, `os_version`, `ip`, `last_seen`
2. Если `service_account_id == 0` и `service_key` задан — привязывает к тенанту
3. Если `rustdesk_id` изменился — обновляет
4. Если `rustdesk_id` пустой, но `hostname` совпадает с устройством в БД — восполняет из device-таблицы
5. Если устройство в device-таблице принадлежит другому тенанту и `service_key` указан — перепривязывает к правильному тенанту и снимает флаг `is_pending`

---

### 5.2 GET /admin/agent/inbox

Получает список непрочитанных уведомлений для данной машины. Вызывается каждые **30 секунд**, после отправки запроса помощи — каждые **5 секунд в течение 60 секунд** (burst polling).

#### Request

```
GET /admin/agent/inbox?machine_id=a3f8c1d2e4b5...&service_key=my-company
```

| Параметр | Тип | Обязательное | Описание |
|----------|-----|:---:|---------|
| `machine_id` | string | ✓ | ID агента |
| `service_key` | string | — | Slug тенанта |

#### Response

**200 OK**
```json
{
  "items": [
    {
      "id":         42,
      "title":      "Плановые работы",
      "body":       "Сервер будет недоступен 15 мая с 03:00 до 05:00.",
      "type":       "banner",
      "options":    [],
      "link":       "",
      "link_label": "",
      "image_url":  "",
      "severity":   "warning"
    }
  ]
}
```

| Поле | Тип | Описание |
|------|-----|---------|
| `id` | int | ID уведомления в БД |
| `title` | string | Заголовок диалога |
| `body` | string | Текст сообщения |
| `type` | string | Тип: `banner`, `poll`, `config_update`, `support_ping` |
| `options` | string[] | Варианты для опроса или action-строки для support_ping |
| `link` | string | Необязательная URL-кнопка |
| `link_label` | string | Текст URL-кнопки |
| `image_url` | string | Необязательное изображение (абсолютный или относительный URL) |
| `severity` | string | `info`, `warning`, `error`, `success` |

**400 Bad Request** — `machine_id` не передан

#### Логика фильтрации

Уведомление попадает в ответ если:
1. `expires_at > now` — не истекло
2. `deliver_after IS NULL OR deliver_after <= now` — время доставки наступило
3. `service_account_id` совпадает с тенантом машины **или** `service_account_id = 0` (системная рассылка)
4. `target_all = true` **или** `machine_id` присутствует в JSON-массиве `target_ids`
5. Уведомление ещё не было доставлено этой машине (нет записи в `agent_notification_delivery`)

Уведомления типа `support_ping` **автоматически помечаются доставленными** при первом получении (чтобы оператор не получил дублирующийся диалог на следующем цикле). Явный ACK происходит после нажатия кнопки действия.

---

### 5.3 POST /admin/agent/notification/{id}/ack

Подтверждает получение уведомления. Идемпотентный — повторные вызовы безопасны.

#### Request

```
POST /admin/agent/notification/42/ack?machine_id=a3f8c1d2e4b5...
```

| Параметр | Тип | Обязательное | Описание |
|----------|-----|:---:|---------|
| `machine_id` | string (query) | ✓ | ID агента |

Тело запроса не требуется.

#### Response

**200 OK**
```json
{ "ok": true }
```

**400 Bad Request** — `machine_id` не передан

---

### 5.4 POST /admin/agent/notification/{id}/vote

Записывает ответ пользователя на опрос (тип `poll`). Если ACK ещё не был отправлен — создаёт запись доставки одновременно с голосом.

#### Request

```
POST /admin/agent/notification/42/vote?machine_id=a3f8c1d2e4b5...
Content-Type: application/json
```

```json
{ "vote": "Вариант А" }
```

| Поле | Тип | Обязательное | Описание |
|------|-----|:---:|---------|
| `vote` | string | ✓ | Строка выбранного варианта из массива `options` |

`machine_id` передаётся как query-параметр (аналогично `/ack`).

#### Response

**200 OK**
```json
{ "ok": true }
```

---

### 5.5 GET /admin/agent/operators

Возвращает список машин тенанта с установленным Smart Agent — для диалога «Запросить помощь». Используется при отображении списка операторов (только брендированные клиенты).

Активными считаются машины, отправившие heartbeat за последние **30 дней**.

#### Request

```
GET /admin/agent/operators?service_key=my-company
```

| Параметр | Тип | Обязательное | Описание |
|----------|-----|:---:|---------|
| `service_key` | string | — | Slug тенанта. При пустом — пустой список |

#### Response

**200 OK**
```json
{
  "items": [
    {
      "machine_id":  "b9d4a2f1c0e3...",
      "rustdesk_id": "987654321",
      "hostname":    "OPERATOR-PC",
      "os":          "windows",
      "last_seen":   "2025-05-30T14:22:00Z",
      "online":      true
    }
  ]
}
```

| Поле | Тип | Описание |
|------|-----|---------|
| `machine_id` | string | ID агента оператора — используется как `target_machine_id` при отправке запроса |
| `rustdesk_id` | string | Числовой RustDesk ID. Пустая строка если не известен |
| `hostname` | string | Имя хоста |
| `os` | string | Операционная система |
| `last_seen` | string (RFC3339) | Время последнего heartbeat |
| `online` | bool | `true` если `last_seen` < 10 минут назад |

Список отсортирован по `last_seen` убыванию, лимит **200** записей.

---

### 5.6 POST /admin/agent/support-request

Отправляет запрос помощи. Создаёт запись `AgentSupportRequest` и доставляет `support_ping` уведомление целевому оператору (или всем операторам тенанта при broadcast).

#### Request

```
POST /admin/agent/support-request
Content-Type: application/json
```

```json
{
  "machine_id":         "a3f8c1d2e4b5...",
  "service_key":        "my-company",
  "hostname":           "DESKTOP-ABC123",
  "message":            "Не работает принтер",
  "target_machine_id":  "b9d4a2f1c0e3...",
  "target_rustdesk_id": "",
  "from_rustdesk_id":   "123456789"
}
```

| Поле | Тип | Обязательное | Описание |
|------|-----|:---:|---------|
| `machine_id` | string | ✓ | ID агента запрашивающей машины |
| `service_key` | string | — | Slug тенанта |
| `hostname` | string | — | Имя хоста пользователя (отображается оператору) |
| `message` | string | — | Описание проблемы |
| `target_machine_id` | string | — | `machine_id` конкретного оператора. Пустая строка = broadcast «Любой свободный» |
| `target_rustdesk_id` | string | — | Альтернатива: числовой RustDesk ID оператора. Сервер сам разрешит в `machine_id` |
| `from_rustdesk_id` | string | — | Числовой RustDesk ID пользователя. Оператор использует для авто-подключения при принятии |

**Логика разрешения цели:**

Если `target_machine_id` пустой, но `target_rustdesk_id` задан — сервер ищет оператора в трёх шагах:
1. `agent_machine.machine_id = target_rustdesk_id` (прямой поиск)
2. `agent_machine.rustdesk_id = target_rustdesk_id`
3. `device.rustdesk_id = target_rustdesk_id` → cross-reference по hostname с `agent_machine`

Пробелы в `target_rustdesk_id` удаляются автоматически (`"123 456 789"` → `"123456789"`).

#### Response

**200 OK** — запрос принят и уведомление доставлено
```json
{ "ok": true, "id": 15 }
```

| Поле | Тип | Описание |
|------|-----|---------|
| `id` | int | ID созданного `AgentSupportRequest` |

**400 Bad Request** — `machine_id` не передан

**422 Unprocessable Entity** — broadcast, но нет других машин в тенанте
```json
{ "ok": false, "error": "NoOperatorsAvailable" }
```

**429 Too Many Requests** — rate limit или открытый лимит
```json
{
  "error":           "rate_limit",
  "message":         "Слишком много запросов. Подождите немного.",
  "retry_after_min": 60
}
```
```json
{
  "error":   "open_limit",
  "message": "У вас уже есть незавершённый запрос. Дождитесь ответа специалиста."
}
```
```json
{
  "error":   "decline_cooldown",
  "message": "Этот специалист недавно отклонил запрос. Попробуйте чуть позже или выберите другого."
}
```

---

### 5.7 POST /admin/agent/support-request/respond

Вызывается агентом **оператора** после нажатия кнопки в диалоге `support_ping`. Обновляет статус запроса и отправляет уведомление обратно пользователю.

#### Request

```
POST /admin/agent/support-request/respond
Content-Type: application/json
```

```json
{
  "machine_id":  "b9d4a2f1c0e3...",
  "service_key": "my-company",
  "request_id":  15,
  "action":      "accept",
  "message":     ""
}
```

| Поле | Тип | Обязательное | Описание |
|------|-----|:---:|---------|
| `machine_id` | string | ✓ | `machine_id` оператора (не пользователя) |
| `service_key` | string | — | Slug тенанта |
| `request_id` | int | ✓ | ID из поля `id` ответа `/support-request` |
| `action` | string | ✓ | `accept`, `defer10`, `defer60`, `decline` |
| `message` | string | — | Необязательное сообщение от оператора пользователю |

**Значения `action`:**

| Значение | Статус запроса | Уведомление пользователю |
|----------|---------------|--------------------------|
| `accept` | `accepted` | «✓ Запрос принят. Специалист скоро подключится.» |
| `defer10` | `deferred` (на 10 мин) | «⏰ Специалист ответит через 10 минут» + напоминание через 10 мин |
| `defer60` | `deferred` (на 60 мин) | «⏰ Специалист ответит через час» + напоминание через 60 мин |
| `decline` | `declined` | «Запрос отклонён. Попробуйте обратиться к другому.» |

#### Response

**200 OK**
```json
{ "ok": true, "status": "accepted" }
```

**400 Bad Request** — `request_id` не передан или `action` неизвестен

**403 Forbidden** — запрос адресован другому оператору (`target_machine_id` не совпадает)
```json
{ "error": "not_your_request" }
```

**404 Not Found** — запрос не найден
```json
{ "error": "not_found" }
```

**409 Conflict** — запрос уже был принят, отклонён или закрыт
```json
{ "error": "already_resolved", "status": "accepted" }
```

---

## 6. Типы уведомлений

Поле `type` в объекте уведомления из `/inbox`:

### `banner`
Информационное сообщение. Агент показывает `AlertDialog` с заголовком, телом и кнопкой «Закрыть». При наличии `link` — дополнительная кнопка «Открыть».

Агент **автоматически ACK'ирует** баннер при показе.

```json
{
  "type":     "banner",
  "title":    "Внимание",
  "body":     "Запланированное обслуживание 25 мая с 03:00",
  "severity": "warning",
  "link":     "https://status.everty.ru",
  "link_label": "Статус системы"
}
```

### `poll`
Опрос с вариантами ответов. Поле `options` — массив строк-вариантов. Диалог нельзя закрыть без выбора (`barrierDismissible: false`).

Агент **автоматически ACK'ирует** при показе, затем вызывает `/vote` при нажатии кнопки.

```json
{
  "type":    "poll",
  "title":   "Оцените качество поддержки",
  "body":    "Как вы оцениваете работу специалиста?",
  "options": ["Отлично", "Хорошо", "Плохо"]
}
```

### `config_update`
Обновление настроек подключения к серверу. Поле `body` — JSON-строка с параметрами.

Агент **автоматически ACK'ирует** и показывает диалог с инструкцией и кнопками копирования.

```json
{
  "type":  "config_update",
  "title": "Обновление настроек",
  "body":  "{\"server\": \"relay.everty.ru\", \"key\": \"ABC123==\", \"api_server\": \"https://desk.everty.ru\"}"
}
```

Структура `body` после `jsonDecode`:

| Поле | Описание |
|------|---------|
| `server` | ID/Relay сервер RustDesk |
| `key` | Публичный ключ (base64) |
| `api_server` | URL API сервера |

### `support_ping`
Входящий запрос помощи для оператора. Агент показывает специальный диалог с кнопками «Принять / Через 10 мин / Через час / Отклонить».

**Не автоматически ACK'ируется** при получении из inbox — ACK происходит только после нажатия кнопки действия. Помечается доставленным в БД немедленно при первом получении, чтобы предотвратить дублирование диалогов при следующем цикле опроса.

```json
{
  "type":    "support_ping",
  "title":   "Запрос помощи",
  "body":    "От: DESKTOP-USER\n\nНе работает принтер",
  "options": [
    "accept:req-15",
    "defer10:req-15",
    "defer60:req-15",
    "decline:req-15",
    "meta:from_rdid=123456789"
  ],
  "severity": "warning"
}
```

### `entitlements_changed`
Сигнал "тариф/подписка изменились — перезапроси `POST /api/currentUser`".
Тело пустое, показывать пользователю ничего не нужно — это чисто
техническое уведомление для клиента. Ставится сервером автоматически при
успешной оплате, при изменении подписки суперадминистратором, либо при
правке самого тарифа (например, добавлении нового `custom_features`-флага
— в этом случае уведомление получают все аккаунты на этом тарифе).

```json
{
  "type": "entitlements_changed",
  "title": "",
  "body": ""
}
```

Агент должен **ack'нуть** уведомление как обычно и сразу вызвать
`POST /api/currentUser`, чтобы обновить `entitlements` (см. R3.1) — не
дожидаясь следующего планового опроса.

---

## 7. Жизненный цикл запроса помощи

```
Пользователь                    Сервер                         Оператор
    |                              |                               |
    |-- POST /support-request ---->|                               |
    |   {target_machine_id: "..."}  |                               |
    |                              |-- INSERT AgentSupportRequest  |
    |                              |-- INSERT AgentNotification    |
    |                              |   (type=support_ping)         |
    |<-- {ok:true, id:15} ---------|                               |
    |                              |                               |
    |                              |<--- GET /inbox ---------------|
    |                              |--- {items:[{type:support_ping}]}->|
    |                              |    (помечает доставленным)    |
    |                              |                               |
    |                              |       [оператор нажимает кнопку]
    |                              |                               |
    |                              |<--- POST /support-request/respond
    |                              |     {action: "accept"}        |
    |                              |-- UPDATE status=accepted      |
    |                              |-- INSERT AgentNotification    |
    |                              |   (banner для пользователя)   |
    |                              |-- POST /notification/X/ack ---|
    |                              |                               |
    |                              |<--- GET /inbox (burst poll)---|
    |<--- {type:banner, ------------------------------------>      |
    |      title:"✓ Запрос принят"}|                               |
    |                              |                               |
```

**Статусы `AgentSupportRequest`:**

| Статус | Описание |
|--------|---------|
| `new` | Создан, оператор ещё не ответил |
| `in_progress` | Оператор обрабатывает |
| `accepted` | Оператор принял — финальный |
| `deferred` | Оператор отложил (defer10/defer60) |
| `declined` | Оператор отклонил — финальный |
| `closed` | Закрыт администратором — финальный |

---

## 8. Коды ошибок

| HTTP | Код ошибки | Описание |
|------|-----------|---------|
| 400 | `machine_id required` | Не передан обязательный `machine_id` |
| 400 | `request_id required` | Не передан `request_id` в `/respond` |
| 400 | `unknown_action` | Неизвестное значение поля `action` |
| 403 | `not_your_request` | Запрос адресован другому оператору |
| 404 | `not_found` | Запрос помощи не существует |
| 409 | `already_resolved` | Запрос уже завершён |
| 422 | `NoOperatorsAvailable` | Broadcast запрос, но других машин в тенанте нет |
| 429 | `rate_limit` | Превышен IP-лимит heartbeat или лимит запросов в час |
| 429 | `open_limit` | Превышено количество одновременных открытых запросов |
| 429 | `decline_cooldown` | Тот же оператор недавно отклонил запрос |

---

## 9. Интервалы опроса и backoff

| Событие | Интервал |
|---------|---------|
| Heartbeat (нормальный режим) | 1 минута |
| Первый heartbeat после старта | через 3 секунды |
| Inbox polling (нормальный режим) | 30 секунд |
| Первый inbox poll после старта | через 8 секунд |
| Burst polling после отправки запроса помощи | каждые 5 сек, 60 секунд |
| Burst polling после ответа на support_ping | каждые 5 сек, 60 секунд |

**Backoff при сетевых ошибках:**

Heartbeat: при неудаче планирует retry через `5 * failures` секунд, максимум 3 быстрых retry. После 3-й неудачи переключается на обычный таймер.

Inbox: при неудаче увеличивает счётчик `_inboxFailures`, продолжает работу по обычному таймеру.

---

## 10. Поле `options` в support_ping

Массив строк, каждая строка — либо **action**, либо **meta-параметр**:

**Action-строка:** `"{action}:{ref}"`
- `action` — одно из `accept`, `defer10`, `defer60`, `decline`
- `ref` — `req-{id}`, где `id` — ID запроса помощи

```
"accept:req-15"
"defer10:req-15"
"defer60:req-15"
"decline:req-15"
```

**Meta-строка:** `"meta:{key}={value}"`
- `meta:from_rdid={rustdesk_id}` — RustDesk ID пользователя для авто-подключения при принятии

```
"meta:from_rdid=123456789"
```

**Пример парсинга в Dart:**
```dart
for (final o in options) {
  if (o.startsWith('meta:from_rdid=')) {
    fromRdId = o.substring('meta:from_rdid='.length).trim();
    continue; // не кнопка
  }
  final parts = o.split(':');
  final action = parts[0];          // "accept"
  final ref = parts[1];             // "req-15"
  final requestId = int.tryParse(ref.replaceFirst('req-', ''));
}
```

---

## 11. Конфигурация Dart-агента

Агент инициализируется вызовом `AgentService.instance.initialize(...)` из `main()` форка RustDesk.

```dart
await AgentService.instance.initialize(
  apiServer:           'https://desk.everty.ru',
  serviceKey:          'my-company',    // slug тенанта
  isGenericClient:     false,           // true = unbranded, скрывает список операторов
  showPeerList:        true,            // false = только ввод ID вручную
  allowSupportRequest: true,            // информационный флаг
  appName:             'Everty Desk',   // APP_NAME из hbb_common (для поиска конфига)
);
```

| Параметр | Тип | Описание |
|----------|-----|---------|
| `apiServer` | string | Базовый URL сервера без trailing slash |
| `serviceKey` | string | Slug тенанта. Пустая строка — не привязан |
| `isGenericClient` | bool | `true` = публичный клиент без тенанта; скрывает список операторов, показывает только ввод по ID |
| `showPeerList` | bool | `false` = всегда показывать форму ввода ID вместо списка операторов |
| `allowSupportRequest` | bool | Информационный, не влияет на runtime поведение |
| `appName` | string | Имя приложения, совпадающее с `APP_NAME` из `hbb_common`. Используется для поиска TOML-конфига RustDesk. Пустое/`"null"`/`"rustdesk"` → `"RustDesk"` |

### Инжекция при сборке (GitHub Actions)

Параметры инжектируются через `sed` в workflow при сборке:

```bash
# Пример из generator-windows.yml
sed -i "s|API_SERVER_PLACEHOLDER|${API_SERVER}|g" lib/agent_service.dart
sed -i "s|SERVICE_KEY_PLACEHOLDER|${SERVICE_KEY}|g" lib/agent_service.dart
```

`API_SERVER` получается динамически из `GET /admin/client-build/get-params?token=...` на сервере перед началом сборки.

---

*Документация актуальна для EvertyDesk API Server v1.x и agent_service.dart из ветки `master` репозитория `vaalimusic/everty-workflows`.*

---

# RustDesk Client API Reference

Стандартный API RustDesk (`/api/*`) — эндпоинты, которые вызывает сам клиент RustDesk в фоновом режиме: авторизация, heartbeat, sysinfo, аудит сессий и адресная книга.

**Base URL:**
```
{api_server}/api/
```

---

## Содержание

0. [Обнаружение параметров подключения](#r0-обнаружение-параметров-подключения)
1. [Аутентификация](#r1-аутентификация)
2. [Авторизация](#r2-авторизация)
   - [POST /api/login](#r21-post-apilogin)
   - [GET /api/login-options](#r22-get-apilogin-options)
   - [POST /api/logout](#r23-post-apilogout)
   - [OIDC (Яндекс) — POST /api/oidc/auth + GET /api/oidc/auth-query](#r24-oidc-яндекс--post-apioidcauth--get-apioidcauth-query)
3. [Пользователь](#r3-пользователь)
   - [POST /api/currentUser](#r31-post-apicurrentuser)
   - [GET /api/users](#r32-get-apiusers)
   - [GET /api/peers](#r33-get-apipeers)
4. [Устройство и сессии](#r4-устройство-и-сессии)
   - [POST /api/heartbeat](#r41-post-apiheartbeat)
   - [POST /api/sysinfo](#r42-post-apísysinfo)
   - [POST /api/audit/conn](#r43-post-apiauditconn)
   - [POST /api/audit/file](#r44-post-apiauditfile)
5. [Адресная книга — модель данных и обзор](#ab0-адресная-книга--модель-данных-и-обзор)
6. [Адресная книга — legacy](#r5-адресная-книга--legacy-формат)
   - [GET /api/ab](#r51-get-apiab)
   - [POST /api/ab](#r52-post-apiab)
7. [Адресная книга — multi-AB](#r6-адресная-книга--multi-ab-формат)
   - [POST /api/ab/personal](#r61-post-apiabpersonal)
   - [POST /api/ab/settings](#r62-post-apiabsettings)
   - [POST /api/ab/shared/profiles](#r63-post-apiabsharedprofiles)
   - [POST /api/ab/peers](#r64-post-apiabpeers)
   - [POST /api/ab/peer/add/{guid}](#r65-post-apiabpeeraddguid)
   - [PUT /api/ab/peer/update/{guid}](#r66-put-apiabpeerupdateguid)
   - [DELETE /api/ab/peer/{guid}](#r67-delete-apiabpeerguid)
   - [POST /api/ab/tags/{guid}](#r68-post-apiabtags guid)
   - [POST /api/ab/tag/add/{guid}](#r69-post-apiabtagaddguid)
   - [PUT /api/ab/tag/update/{guid}](#r610-put-apiabtagupdateguid)
   - [PUT /api/ab/tag/rename/{guid}](#r611-put-apiabtagrenameguid)
   - [DELETE /api/ab/tag/{guid}](#r612-delete-apiabtagguid)

---

## R0. Обнаружение параметров подключения

Прежде чем клиент вообще сможет что-то вызывать, ему нужны три вещи: адрес ID-сервера (hbbs), адрес relay-сервера (hbbr) и публичный ключ. Это НЕ часть стокового `/api/*` — отдельный публичный эндпоинт.

```
GET /public/connection
```

Авторизация не требуется, но `public_key` отдаётся только если в запросе есть валидный **admin**-токен (см. R1) — анонимным вызовам возвращается пустая строка (чтобы ключ нельзя было анонимно вытащить и пользоваться relay без учёта тарифа).

**Response: 200 OK** (анонимный вызов)
```json
{
  "public_url": "https://desk.everty.ru",
  "api_url": "https://desk.everty.ru",
  "id_server": "desk.everty.ru",
  "relay_server": "desk.everty.ru",
  "public_key": "",
  "download_windows": "",
  "download_macos": "",
  "download_linux": "",
  "is_self_hosted": false,
  "site_name": "",
  "site_logo": "",
  "status": "ok"
}
```

| Поле | Описание |
|------|----------|
| `api_url` | Базовый URL этого API (без `/api` — клиент дописывает суффикс сам) |
| `id_server` | Адрес hbbs (ID-сервер) — то же значение, что кладётся в `server` конфига клиента |
| `relay_server` | Адрес hbbr (relay-сервер) |
| `public_key` | ed25519-публичный ключ hbbs в base64, нужен клиенту для шифрования handshake — пусто без admin-токена |
| `is_self_hosted` | `true` на self-hosted инсталляциях (Everty Desk On-Premise) — влияет на брендинг (`site_name`/`site_logo`), не на протокол |

На практике: обычный клиент получает `id_server`/`relay_server`/`api_server` не через этот эндпоинт в рантайме, а **зашитыми в конфиг при сборке** (`RustDesk2.toml` — см. `docs/rustdesk-edesk-pro/branded-client.md`), тем же способом, что и стоковый RustDesk. `GET /public/connection` полезен для: (а) диагностики/самопроверки клиента после установки, (б) UI типа «настройки сервера» в кабинете, откуда админ копирует эти три значения вручную.

---

## R1. Аутентификация

Эндпоинты делятся на три группы:

| Группа | Эндпоинты | Заголовок |
|--------|-----------|-----------|
| **Публичные** | `POST /login`, `GET /login-options`, `POST /heartbeat`, `POST /sysinfo`, `POST /audit/*` | Не требуется |
| **Аутентифицированные** | Всё остальное | `Authorization: Bearer <token>` |

### Формат заголовка

Токен передаётся **с префиксом `Bearer `** — это обязательно:
```
Authorization: Bearer eyJhbGciOi...
```

Middleware использует `jwt.FromHeader()` из пакета iris, который ожидает именно `Bearer <token>` и автоматически отрезает префикс перед поиском в БД. Без `Bearer ` функция вернёт пустую строку → сервер ответит `401`.

> **Отличие от `/admin/*`**: там middleware использует `context.GetHeader("Authorization")` напрямую — токен передаётся **без `Bearer`**:
> ```
> Authorization: eyJhbGciOi...   ← только для /admin/*
> ```

### Токены не взаимозаменяемы

Сервер хранит два типа записей в таблице `auth_token`, различаемых флагом `is_admin`:

| Токен получен от | `is_admin` | Работает на |
|------------------|:---:|-------------|
| `POST /api/login` | `false` | `/api/*` |
| `POST /admin/auth/login` | `true` | `/admin/*` |

Использование admin-токена на `/api/*` даёт `401 Unauthorized` — и наоборот.

### Срок жизни

Токен действует **2 часа**. Каждый аутентифицированный запрос, сделанный менее чем за 5 минут до истечения, автоматически продлевает токен ещё на 2 часа.

---

## R2. Авторизация

### R2.1 POST /api/login

Универсальный эндпоинт авторизации. Поддерживает несколько режимов в зависимости от поля `type`.

#### Request

```
POST /api/login
Content-Type: application/json
```

```json
{
  "username": "operator1",
  "password": "secret",
  "id":       "123456789",
  "uuid":     "550e8400-e29b-41d4-a716-446655440000",
  "autoLogin": true,
  "type":     "account",
  "deviceInfo": {
    "os":   "Windows",
    "type": "PC",
    "name": "DESKTOP-ABC"
  }
}
```

| Поле | Тип | Описание |
|------|-----|---------|
| `username` | string | Логин пользователя |
| `password` | string | Пароль |
| `id` | string | RustDesk numeric ID устройства |
| `uuid` | string | UUID устройства (fallback для `id`) |
| `autoLogin` | bool | Флаг автологина |
| `type` | string | Режим: `account`, `email_code`, `tfa_code` |
| `verificationCode` | string | Код из email (при `type=email_code`) |
| `tfaCode` | string | Код 2FA (при `type=email_code` + 2FA) |
| `secret` | string | UUID сессии верификации (из предыдущего ответа) |
| `deviceInfo` | object | Информация об устройстве |

**Режимы `type`:**

| Значение | Описание |
|----------|---------|
| `account` | Обычный логин по username + password |
| `email_code` | Двухшаговый: сначала логин (получить `secret`), затем повторный запрос с `verificationCode` |
| `tfa_code` | Аналогично email_code но `verificationCode == tfaCode` |

#### Response — успешный логин

```json
{
  "access_token": "eyJhbGciOi...",
  "type": "access_token",
  "user": {
    "name":     "Иван Петров",
    "email":    "ivan@example.com",
    "note":     "",
    "status":   1,
    "is_admin": false
  }
}
```

#### Response — требуется email-верификация (шаг 1)

```json
{
  "type":     "email_check",
  "tfa_type": "email_check",
  "secret":   "550e8400-e29b-41d4-a716-446655440000"
}
```

После получения `secret` — отправить второй запрос с `type=email_code`, `verificationCode=<код из письма>`, `secret=<полученный secret>`.

#### Response — требуется 2FA (шаг 1)

```json
{
  "type":     "email_check",
  "tfa_type": "tfa_check",
  "secret":   "550e8400-e29b-41d4-a716-446655440001"
}
```

После — отправить запрос с `type=email_code`, `verificationCode=<TOTP-код>`, `tfaCode=<тот же код>`, `secret=<secret>`.

#### Response — ошибка

```json
{ "error": "Username Or Password Error" }
```

#### Побочный эффект

При успешном логине создаётся запись в `system_events` с типом `operator.login`. При ошибке — `operator.login.failed`.

---

### R2.2 GET /api/login-options

Возвращает список включённых OIDC-провайдеров в формате, который ожидает стоковый RustDesk-клиент (`["oidc/<name>", ...]`).

```
GET /api/login-options
```

**Response: 200 OK**
```json
["oidc/yandex"]
```

Если в суперадминке не настроен/не включён ни один SSO-провайдер — возвращает пустой массив `[]`. Сейчас единственный поддерживаемый провайдер — `yandex` (см. R2.4). Клиент должен рисовать кнопку входа только для провайдеров, реально присутствующих в этом списке — не хардкодить набор заранее.

---

### R2.3 POST /api/logout

Инвалидирует текущий токен.

#### Request

```
POST /api/logout
Authorization: <token>
Content-Type: application/json
```

```json
{ "id": "123456789" }
```

| Поле | Тип | Описание |
|------|-----|---------|
| `id` | string | RustDesk ID устройства. Должен совпадать с тем, что был при логине. Можно передать пустую строку. |

**Response: 200 OK**
```
ok
```

**Response: 403** — `id` не совпадает с токеном

---

### R2.4 OIDC (Яндекс) — POST /api/oidc/auth + GET /api/oidc/auth-query

Это единственный путь входа через Яндекс, который реально совместим с протоколом стокового RustDesk-клиента (`bind.mainAccountAuth` → `OidcAuthUrl` в `src/hbbs_http/account.rs`). Клиент запускает флоу, открывает `url` в системном браузере и параллельно опрашивает `auth-query`, пока пользователь не подтвердит вход у Яндекса.

> ⚠️ В коде backend'а (`backend/app/controller/admin/sso.go`) есть ещё один, отдельный флоу — `POST /admin/sso/yandex/device-code` + `POST /admin/sso/yandex/device-poll` (RFC 8628 device-code напрямую против Яндекса). Он выдаёт **admin**-токен (`is_admin=true`), который не работает ни с одним эндпоинтом из этого документа (все они требуют `is_admin=false`, см. R1). Не используйте этот путь для клиента — он существует для другого сценария и не связан с протоколом ниже.

#### Шаг 1 — начать флоу

```
POST /api/oidc/auth
Content-Type: application/json
```

```json
{
  "op": "yandex",
  "id": "123456789",
  "uuid": "550e8400-e29b-41d4-a716-446655440000",
  "deviceInfo": { "os": "Windows", "type": "PC", "name": "DESKTOP-ABC" }
}
```

| Поле | Тип | Описание |
|------|-----|---------|
| `op` | string | Провайдер — сейчас только `"yandex"`. Другое значение → `{"error":"unsupported_provider"}` |
| `id` | string | RustDesk numeric ID устройства |
| `uuid` | string | UUID устройства |
| `deviceInfo` | object | Произвольная информация об устройстве, не валидируется |

**Response: 200 OK**
```json
{
  "code": "3f1a9c2e8b7d4f0a1c6e9b2d5a8f3c7e",
  "url": "https://oauth.yandex.ru/authorize?response_type=code&client_id=...&state=3f1a9c2e8b7d4f0a1c6e9b2d5a8f3c7e"
}
```

Клиент должен открыть `url` в системном браузере (не встраивать в webview — так делает и сам RustDesk). Сессия живёт 10 минут (`oidcSessionTTL`) — после этого `code` протухает.

**Response — SSO не настроен на сервере**
```json
{ "error": "yandex_not_configured" }
```

#### Шаг 2 — пользователь авторизуется у Яндекса в браузере

Браузер редиректит на `GET /admin/sso/yandex/callback?code=...&state=<code из шага 1>` — этим занимается сервер, клиенту в этом шаге ничего делать не нужно, кроме как ждать (опрашивая шаг 3). После успешной авторизации сервер:
- находит/создаёт пользователя по Яндекс-профилю (новый Яндекс-логин без привязки к существующему email автоматически получает свой личный `ServiceAccount` с адресной книгой и постоянным личным тарифом — см. `findOrCreateUserByYandex`);
- выпускает клиентский `AuthToken` (`is_admin=false`, привязан к переданным `id`/`uuid`, срок жизни 90 дней);
- показывает в браузере статичную страницу «вход выполнен, можно закрыть вкладку».

#### Шаг 3 — опрос результата

```
GET /api/oidc/auth-query?code=3f1a9c2e8b7d4f0a1c6e9b2d5a8f3c7e
```

Опрашивать примерно раз в секунду, пока не авторизуется. Пока пользователь не завершил вход в браузере:
```json
{ "error": "No authed oidc is found" }
```
Текст ошибки должен совпадать **дословно** — это форма, которую ожидает Rust-клиент, чтобы отличить «ещё не готово, продолжай опрашивать» от настоящей ошибки.

Если сессия протухла (прошло больше 10 минут с шага 1):
```json
{ "error": "oidc session expired, please try again" }
```

**Response при успехе** (одноразовая выдача — токен из сессии стирается сразу после первого успешного ответа, повторный запрос с тем же `code` снова вернёт «No authed oidc is found»):
```json
{
  "access_token": "eyJhbGciOi...",
  "type": "access_token",
  "tfa_type": "",
  "secret": "",
  "user": {
    "name": "Иван Петров",
    "display_name": "Иван Петров",
    "email": "ivan@example.com",
    "note": "",
    "status": 1,
    "info": {},
    "is_admin": false,
    "third_auth_type": "yandex"
  }
}
```

`access_token` — обычный клиентский токен (`is_admin=false`), используется дальше как в R1: `Authorization: Bearer <access_token>` на всех остальных эндпоинтах `/api/*`, включая адресную книгу (R5/R6).

---

## R3. Пользователь

### R3.1 POST /api/currentUser

Возвращает информацию о текущем авторизованном пользователе — в том числе
`entitlements`, плоскую мапу тарифных фич-флагов. **Это основная точка входа
для gating функций в стороннем клиенте по тарифу** (вход через логин уже
даёт токен — этим эндпоинтом клиент узнаёт, что конкретно этому пользователю
разрешено).

```
POST /api/currentUser
Authorization: <token>
```

**Response: 200 OK**
```json
{
  "name":     "Иван Петров",
  "email":    "ivan@example.com",
  "note":     "",
  "status":   1,
  "is_admin": false,
  "entitlements": {
    "has_smart_agent": true,
    "has_ldap": false,
    "has_yandex_sso": true,
    "has_client_builder": false,
    "has_branded_client": false,
    "has_invoice_billing": true,
    "has_audit": false,
    "has_priority_support": false,
    "vm_mode": "true",
    "max_vm_slots": "5"
  }
}
```

`entitlements` собирается сервером (`service.ResolveEntitlements`,
`backend/app/service/commercial.go`) из активной подписки сервис-аккаунта
пользователя:

- Типизированные флаги тарифа (`has_smart_agent`, `has_ldap`,
  `has_yandex_sso`, `has_client_builder`, `has_branded_client`,
  `has_invoice_billing`, `has_audit`, `has_priority_support`) — булевы,
  всегда присутствуют.
- Плюс **произвольные** ключи из `TariffPlan.CustomFeatures` — JSON-объект
  `string → string`, который суперадмин свободно редактирует в форме
  тарифа (список ключ/значение, без правок бэкенда на каждую новую фичу).
  Значения — всегда строки (`"true"`/`"false"` для вкл/выкл, либо
  произвольное число/строка для лимитов) — клиент сам разбирает их смысл.

**Важно для клиента:**
- `entitlements` возвращается **пустым объектом `{}`** только если у
  сервис-аккаунта вообще нет подписки/тарифа. Статус подписки
  (`past_due`, `canceled`, просроченный период) на `entitlements`
  **не влияет** — это осознанное решение, совпадающее с уже существующей
  в проекте конвенцией "мягкой" проверки фич (`service.CheckFeature`,
  использует LDAP/выставление счетов/брендированный клиент): разовый сбой
  оплаты не должен мгновенно отбирать доступ к фиче. Это отличается от
  лимита устройств (`CanAddDevice`), который жёстко блокирует новые
  устройства в тех же статусах — там это лимит потребления ресурса, а не
  фича-гейт, и разница задумана.
- `entitlements` обновляется push'ем через Smart Agent inbox — при
  реальном изменении подписки/тарифа сервер кладёт в инбокс уведомление
  типа `entitlements_changed` (см. раздел 6 "Типы уведомлений" выше);
  получив его, клиент должен сразу перезапросить `currentUser`, а не
  полагаться только на периодический опрос.
- Это модель "клиент доверяет ответу сервера" — сервер не проверяет
  реальное использование гейтуемой фичи на своей стороне (в отличие от,
  скажем, лимита устройств в heartbeat). Если фича критична для защиты от
  обхода — рассмотрите отдельную серверную проверку в момент её
  использования, а не только флаг в `entitlements`.
- **Соглашение об именовании произвольных ключей** — используйте
  `snake_case` латиницей (`vm_mode`, `max_vm_slots`), ведите единый список
  используемых ключей и их значений отдельно (например, в этом файле или
  в трекере задач) — сервер никак не проверяет согласованность имён между
  тарифами, опечатка в одном тарифе тихо разойдётся с тем, что проверяет
  клиент.

---

### R3.2 GET /api/users

Список пользователей тенанта. Требует прав администратора (`is_admin = true`).

```
GET /api/users?current=1&pageSize=10&status=1
Authorization: <token>
```

| Параметр | Тип | По умолчанию | Описание |
|----------|-----|:---:|---------|
| `current` | int | 1 | Номер страницы |
| `pageSize` | int | 10 | Размер страницы |
| `status` | int | 1 | Фильтр по статусу пользователя |

**Response: 200 OK**
```json
{
  "total": 5,
  "data": [
    {
      "name":     "Иван Петров",
      "email":    "ivan@example.com",
      "note":     "",
      "status":   1,
      "is_admin": false
    }
  ]
}
```

**Response: 403** — пользователь не является администратором
```json
{ "error": "Admin required!" }
```

---

### R3.3 GET /api/peers

Список устройств в адресной книге текущего пользователя (плоский список без привязки к конкретной AB).

```
GET /api/peers?current=1&pageSize=10
Authorization: <token>
```

**Response: 200 OK**
```json
{
  "total": 3,
  "data": [
    {
      "id":        "123456789",
      "info": {
        "username":    "user",
        "os":          "Windows",
        "device_name": "DESKTOP-ABC"
      },
      "status":    1,
      "user":      "operator1",
      "user_name": "operator1"
    }
  ]
}
```

---

## R4. Устройство и сессии

Эти эндпоинты вызывает сам RustDesk-клиент автоматически. Авторизация **не требуется** — идентификация происходит по полю `id` (RustDesk numeric ID).

### R4.1 POST /api/heartbeat

Периодический сигнал «я живой» от клиента RustDesk. Создаёт устройство при первом обращении (в `pending` пул) или обновляет статус существующего.

```
POST /api/heartbeat
Content-Type: application/json
```

```json
{
  "id":          "123456789",
  "uuid":        "550e8400-e29b-41d4-a716-446655440000",
  "modified_at": 1725698100,
  "ver":         1002070,
  "version":     "1.2.7",
  "conns":       [762, 763]
}
```

| Поле | Тип | Описание |
|------|-----|---------|
| `id` | string | RustDesk numeric ID. Если пустой — используется `uuid` |
| `uuid` | string | UUID устройства (fallback) |
| `modified_at` | int64 | Unix-время последнего изменения конфига |
| `ver` | int64 | Версия в виде числа (например `1002070` = `1.2.70`) |
| `version` | string | Версия строкой (например `"1.2.7"`) |
| `conns` | int[] | Массив активных conn_id подключений |

**Response: 200 OK**
```json
{
  "modified_at": 1725698200,
  "strategy":    {}
}
```

Поле `strategy` — JSON с флагами возможностей клиента (зависит от версии).

**Response: ошибка превышения лимита**
```json
{
  "error": "device limit exceeded",
  "used":  25,
  "limit": 25
}
```

#### Поведение при первом обращении

Новое устройство создаётся в `pending` пуле (`is_pending = true`) дефолтного сервисного аккаунта. Чтобы оно появилось в нужном тенанте — Smart Agent должен отправить heartbeat с `service_key`, который автоматически привяжет устройство.

---

### R4.2 POST /api/sysinfo

Подробная информация о системе устройства. Вызывается RustDesk после успешного подключения.

```
POST /api/sysinfo
Content-Type: application/json
```

```json
{
  "id":       "123456789",
  "uuid":     "550e8400-...",
  "cpu":      "Intel Core i7-12700K",
  "hostname": "DESKTOP-ABC123",
  "memory":   "16384 MB",
  "os":       "Windows 11 Pro",
  "username": "user",
  "version":  "1.2.7"
}
```

| Поле | Тип | Описание |
|------|-----|---------|
| `id` | string | RustDesk ID |
| `uuid` | string | UUID (fallback) |
| `cpu` | string | Модель процессора |
| `hostname` | string | Имя компьютера |
| `memory` | string | Объём RAM |
| `os` | string | Версия ОС |
| `username` | string | Текущий пользователь ОС |
| `version` | string | Версия RustDesk |

**Response: 200 OK**
```
SYSINFO_UPDATED
```

**Response: устройство не найдено**
```
ID_NOT_FOUND
```

---

### R4.3 POST /api/audit/conn

Событие подключения/отключения. Вызывается RustDesk при каждом изменении состояния сессии. Один и тот же эндпоинт принимает три разных типа тела запроса.

```
POST /api/audit/conn
Content-Type: application/json
```

**Тип 1 — новое подключение (`action=new`):**
```json
{
  "action":     "new",
  "conn_id":    762,
  "id":         "123456789",
  "ip":         "192.168.1.100",
  "session_id": 0,
  "uuid":       "550e8400-..."
}
```

**Тип 2 — закрытие подключения (`action=close`):**
```json
{
  "action":     "close",
  "conn_id":    762,
  "id":         "123456789",
  "session_id": 17409556129324805845,
  "uuid":       "550e8400-..."
}
```

**Тип 3 — привязка peer (без `action`, есть `peer`):**
```json
{
  "conn_id":    762,
  "id":         "123456789",
  "peer":       ["987654321", "SYSTEM"],
  "session_id": 17409556129324805845,
  "type":       0,
  "uuid":       "550e8400-..."
}
```

| Поле `type` | Значение |
|-------------|---------|
| `0` | Сессия управления (control) |
| `1` | Передача файлов |
| `2` | TCP-туннель |

**Тип 4 — обновление заметки (есть `note`):**
```json
{
  "id":         "123456789",
  "session_id": "17409556129324805845",
  "note":       "Ручная заметка оператора"
}
```

**Response: 200 OK** — пустое тело для всех вариантов.

---

### R4.4 POST /api/audit/file

Событие передачи файла.

```
POST /api/audit/file
Content-Type: application/json
```

```json
{
  "id":      "123456789",
  "peer_id": "987654321",
  "path":    "/Users/user/Downloads/report.pdf",
  "is_file": true,
  "type":    1,
  "uuid":    "550e8400-...",
  "info":    "{\"files\":[[\"report.pdf\",1048576]],\"ip\":\"192.168.1.100\",\"name\":\"user\",\"num\":1}"
}
```

| Поле `type` | Значение |
|-------------|---------|
| `0` | Скачивание с управляемого устройства на оператора |
| `1` | Загрузка с оператора на управляемое устройство |

**Response: 200 OK** — пустое тело.

---

## AB0. Адресная книга — модель данных и обзор

Прежде чем читать R5/R6 построчно — вот как это устроено концептуально.
Структура **плоская, без вложенных папок/подпапок** — то, что выглядит
как "вложенность", на самом деле двухуровневая связь: адресная книга →
пиры, плюс отдельный список тегов, которые пришпиливаются к пирам как
плоский список имён (не дерево).

### Сущности

```
AddressBook (guid)  ──1:N──  Peer (ab_id)
      │                        │
      │                        └─ tags: ["prod", "vip"]  (просто массив строк)
      │
      └─ AddressBookTag (ab_id, name, color)   — определения тегов для этой AB
```

- **`AddressBook`** (`backend/app/model/address_book.go:9`) — одна запись =
  одна адресная книга. Поля: `guid` (публичный идентификатор, им клиент
  оперирует во всех multi-AB запросах — `/api/ab/peers?ab=<guid>` и т.д.),
  `user_id` (владелец), `name`, `owner` (имя владельца строкой, для
  отображения), `note`, `rule`, `max_peer`, `shared`.
- **`Peer`** (`backend/app/model/peer.go:5`) — одно устройство в одной
  конкретной адресной книге (`ab_id`). Поля `tags` (JSON-массив имён тегов
  строкой) и `note` — простой текст, не структурированные данные.
  RDP-поля (`rdpPort`, `rdpUsername`, `loginName`, `sameServer`,
  `forceAlwaysRelay`) — расширения этого проекта поверх стокового
  RustDesk-формата.
- **Теги — ДВЕ параллельные таблицы**, важно не перепутать:
  - `Tags` (`backend/app/model/tags.go`) — legacy, `user_id`-scoped
    (без привязки к конкретной AB), используется только в R5
    (`GET/POST /api/ab`). `Color` хранится строкой.
  - `AddressBookTag` (`backend/app/model/address_book.go:27`) — новый
    формат, `ab_id`-scoped (теги привязаны к конкретной адресной книге, а
    не ко всему аккаунту), используется в R6.8-R6.12. `Color` хранится
    как `int64` (packed RGBA, не строка).
  - Сам `Peer.tags` в обоих форматах — просто список **имён** тегов;
    цвет/метаданные тега подтягиваются отдельно из соответствующей
    таблицы по имени.

### Личная адресная книга — авто-создание

`POST /api/ab/personal` (`address_book.go:249`) идемпотентно
создаёт-или-возвращает личную AB пользователя при первом обращении:
`name = "My address book"` (`model.PersonalAddressBookName`), `rule = 3`
(full control), `max_peer` = `model.MaxPeer` = `0` (означает "без лимита
на уровне AB" — реальный лимит устройств контролируется тарифом
сервис-аккаунта, не этим полем).

### `rule` — уровень доступа (1/2/3)

`1` = read, `2` = read&write, `3` = full control — поле присутствует на
`AddressBook` и отдаётся в списке "общих" адресных книг
(`POST /api/ab/shared/profiles`, R6.3), которые есть у других
пользователей того же service-аккаунта (`shared = true`).

**Важный нюанс для интеграции**: сам `rule` сейчас **не проверяется** в
эндпоинтах записи (`ab/peer/add|update/{guid}`,
`ab/peer/{guid}` DELETE — см. `ab_peer.go:35,121,224,322`) — там везде
жёсткое условие `Where("user_id = ? and guid = ?", user.Id, abGuid)`,
т.е. писать в пиры может только сам владелец AB. `shared`/`rule`
сегодня — это только **список метаданных** ("вот какие AB существуют у
коллег и какой у них номинальный уровень доступа"), а не реально
работающая ACL для кросс-пользовательской записи. Если разрабатываемый
клиент рассчитывает на то, что оператор с `rule=2` сможет писать в чужую
"общую" адресную книгу — это придётся либо реализовать на бэкенде
отдельно, либо не полагаться на это в клиенте.

### Legacy vs multi-AB — когда какой использовать

- **Legacy** (R5, `GET/POST /api/ab`) — один цельный JSON-blob на весь
  аккаунт (все теги + все пиры одним объектом). Так работали клиенты
  RustDesk < 1.2.0. Пишется через `PostAb` полной заменой (удаляет все
  старые `Tags`/`Peer` пользователя и вставляет заново — не инкрементально).
- **Multi-AB** (R6) — guid-адресуемые отдельные адресные книги,
  постраничная синхронизация пиров, раздельные теги на AB. Так работают
  современные клиенты. Новый клиент должен использовать multi-AB.

---

## R5. Адресная книга — legacy формат

Устаревший API, совместимый с RustDesk < 1.2.0. Хранит все peer'ы и теги в одном JSON-blob. Используйте multi-AB API (раздел R6) для новых клиентов.

### R5.1 GET /api/ab

Получить всю адресную книгу пользователя.

```
GET /api/ab
Authorization: <token>
```

**Response: 200 OK**
```json
{
  "licensed_devices": 0,
  "data": "{\"tags\":[\"work\",\"home\"],\"peers\":[{\"id\":\"123456789\",\"hash\":\"\",\"username\":\"user\",\"hostname\":\"PC-01\",\"platform\":\"Windows\",\"alias\":\"Офис\",\"tags\":[\"work\"]}],\"tag_colors\":\"{\\\"work\\\":4294901760}\"}"
}
```

| Поле | Тип | Описание |
|------|-----|---------|
| `licensed_devices` | int | Лимит устройств пользователя (0 = нет ограничений) |
| `data` | string | JSON-строка с полем `tags` (string[]), `peers` (object[]) и `tag_colors` (JSON-строка в строке) |

**Структура `data` после `jsonDecode`:**

```json
{
  "tags": ["work", "home"],
  "peers": [
    {
      "id":       "123456789",
      "hash":     "",
      "username": "user",
      "hostname": "PC-01",
      "platform": "Windows",
      "alias":    "Офис",
      "tags":     ["work"]
    }
  ],
  "tag_colors": "{\"work\":4294901760}"
}
```

`tag_colors` — JSON-строка внутри строки: `{"tag_name": ARGB_int64}`.

---

### R5.2 POST /api/ab

Полная замена адресной книги (delete-all + insert). Транзакционная операция.

```
POST /api/ab
Authorization: <token>
Content-Type: application/json
```

```json
{
  "data": "{\"tags\":[\"work\"],\"peers\":[{\"id\":\"123456789\",\"hash\":\"\",\"username\":\"user\",\"hostname\":\"PC-01\",\"platform\":\"Windows\",\"alias\":\"Офис\",\"tags\":[\"work\"]}],\"tag_colors\":\"{\\\"work\\\":4294901760}\"}"
}
```

Поле `data` — строка с тем же форматом что и в GET /api/ab.

**Ограничение:** если у пользователя установлен `licensed_devices > 0` и число peer'ов превышает его — запрос отклоняется:
```json
{ "error": "Number of equipment in excess of licenses" }
```

**Response: 200 OK** — пустое тело при успехе.

#### Побочный эффект

При синхронизации каждый peer автоматически создаёт или обновляет запись в таблице `device` тенанта (через `ensureDeviceFromPeer`). Новые устройства создаются только если тарифный лимит не превышен.

---

## R6. Адресная книга — multi-AB формат

Актуальный API для RustDesk 1.2.0+. Поддерживает несколько адресных книг у одного пользователя. Каждая книга идентифицируется по `guid`.

**Общий порядок работы:**
1. `POST /api/ab/personal` → получить `guid` личной AB
2. `POST /api/ab/peers?ab={guid}` → получить список устройств
3. `POST /api/ab/peer/add/{guid}` → добавить устройство
4. `PUT /api/ab/peer/update/{guid}` → обновить поля
5. `DELETE /api/ab/peer/{guid}` → удалить

---

### R6.1 POST /api/ab/personal

Получить GUID личной адресной книги пользователя. Создаёт её, если ещё нет.

```
POST /api/ab/personal
Authorization: <token>
```

**Response: 200 OK**
```json
{ "guid": "3fa85f64-5717-4562-b3fc-2c963f66afa6" }
```

---

### R6.2 POST /api/ab/settings

Настройки адресной книги — максимальное число peer'ов.

```
POST /api/ab/settings
Authorization: <token>
```

**Response: 200 OK**
```json
{ "max_peer_one_ab": 100 }
```

---

### R6.3 POST /api/ab/shared/profiles

Список общих адресных книг тенанта (созданных другими пользователями с флагом `shared = true`).

```
POST /api/ab/shared/profiles?current=1&pageSize=10
Authorization: <token>
```

**Response: 200 OK**
```json
{
  "total": 2,
  "data": [
    {
      "guid":  "550e8400-e29b-41d4-a716-446655440001",
      "name":  "Общая книга отдела",
      "owner": "admin",
      "note":  "Для всех операторов",
      "rule":  3
    }
  ]
}
```

| Поле `rule` | Значение |
|-------------|---------|
| `1` | Только чтение |
| `2` | Чтение и запись |
| `3` | Полный контроль |

---

### R6.4 POST /api/ab/peers

Список устройств в конкретной адресной книге. Пагинация.

```
POST /api/ab/peers?ab={guid}&current=1&pageSize=10
Authorization: <token>
```

| Параметр | Тип | Описание |
|----------|-----|---------|
| `ab` | string (query) | GUID адресной книги |
| `current` | int | Страница (default: 1) |
| `pageSize` | int | Размер страницы (default: 10) |

**Response: 200 OK**
```json
{
  "total": 5,
  "data": [
    {
      "id":               "123456789",
      "hash":             "",
      "password":         "",
      "username":         "user",
      "hostname":         "DESKTOP-ABC",
      "platform":         "Windows",
      "alias":            "Офис 1",
      "tags":             ["work"],
      "forceAlwaysRelay": "false",
      "rdpPort":          "",
      "rdpUsername":      "",
      "loginName":        "",
      "same_server":      false
    }
  ]
}
```

---

### R6.5 POST /api/ab/peer/add/{guid}

Добавить устройство в адресную книгу.

```
POST /api/ab/peer/add/3fa85f64-5717-4562-b3fc-2c963f66afa6
Authorization: <token>
Content-Type: application/json
```

```json
{
  "id":               "123456789",
  "username":         "user",
  "hostname":         "DESKTOP-ABC",
  "platform":         "Windows",
  "alias":            "Офис 1",
  "tags":             ["work"],
  "forceAlwaysRelay": "false",
  "rdpPort":          "",
  "rdpUsername":      "",
  "loginName":        "",
  "same_server":      ""
}
```

| Поле | Тип | Описание |
|------|-----|---------|
| `id` | string | RustDesk numeric ID устройства |
| `username` | string | Пользователь ОС на устройстве |
| `hostname` | string | Имя компьютера |
| `platform` | string | ОС: `Windows`, `Linux`, `Mac` |
| `alias` | string | Псевдоним в адресной книге |
| `tags` | string[] | Список тегов |
| `forceAlwaysRelay` | string | `"true"` / `"false"` |
| `rdpPort` | string | Порт RDP (если нужен) |
| `rdpUsername` | string | Пользователь RDP |
| `loginName` | string | Логин на устройстве |
| `same_server` | string | Непустая строка = устройство на том же сервере |

**Response: 200 OK** — пустое тело.

**Response: превышен лимит**
```json
{ "error": "exceed_max_devices" }
```

#### Побочный эффект

Автоматически создаёт или обновляет запись в `device` таблице тенанта.

---

### R6.6 PUT /api/ab/peer/update/{guid}

Частичное обновление peer'а. Тело — JSON с только теми полями, которые нужно изменить.

```
PUT /api/ab/peer/update/3fa85f64-5717-4562-b3fc-2c963f66afa6
Authorization: <token>
Content-Type: application/json
```

```json
{
  "id":    "123456789",
  "alias": "Новое имя",
  "tags":  ["work", "priority"]
}
```

Обновляемые поля: `tags`, `alias`, `hash`, `password`. Остальные поля игнорируются.

**Response: 200 OK** — пустое тело.

**Response: 404**
```json
{ "error": "peer not found" }
```

---

### R6.7 DELETE /api/ab/peer/{guid}

Удалить одно или несколько устройств из адресной книги.

```
DELETE /api/ab/peer/3fa85f64-5717-4562-b3fc-2c963f66afa6
Authorization: <token>
Content-Type: application/json
```

Тело — JSON-массив RustDesk ID:
```json
["123456789", "987654321"]
```

**Response: 200 OK** — пустое тело.

---

### R6.8 POST /api/ab/tags/{guid}

Список тегов адресной книги.

```
POST /api/ab/tags/3fa85f64-5717-4562-b3fc-2c963f66afa6
Authorization: <token>
```

**Response: 200 OK**
```json
[
  { "name": "work",  "color": 4294901760 },
  { "name": "home",  "color": 4278190335 }
]
```

`color` — ARGB-значение цвета в виде int64.

---

### R6.9 POST /api/ab/tag/add/{guid}

Создать тег.

```
POST /api/ab/tag/add/3fa85f64-5717-4562-b3fc-2c963f66afa6
Authorization: <token>
Content-Type: application/json
```

```json
{ "name": "vip", "color": 4294901760 }
```

**Response: 200 OK** — пустое тело.

---

### R6.10 PUT /api/ab/tag/update/{guid}

Обновить цвет тега (поиск по имени).

```
PUT /api/ab/tag/update/3fa85f64-5717-4562-b3fc-2c963f66afa6
Authorization: <token>
Content-Type: application/json
```

```json
{ "name": "vip", "color": 4278190335 }
```

**Response: 200 OK** — пустое тело.

---

### R6.11 PUT /api/ab/tag/rename/{guid}

Переименовать тег.

```
PUT /api/ab/tag/rename/3fa85f64-5717-4562-b3fc-2c963f66afa6
Authorization: <token>
Content-Type: application/json
```

```json
{ "old": "work", "new": "office" }
```

**Response: 200 OK** — пустое тело.

---

### R6.12 DELETE /api/ab/tag/{guid}

Удалить один или несколько тегов.

```
DELETE /api/ab/tag/3fa85f64-5717-4562-b3fc-2c963f66afa6
Authorization: <token>
Content-Type: application/json
```

Тело — JSON-массив имён тегов:
```json
["work", "home"]
```

**Response: 200 OK** — пустое тело.

---

## Сводная таблица эндпоинтов

Авторизация `✓` = заголовок `Authorization: Bearer <token>` (токен от `POST /api/login`, `is_admin=false`).

| Метод | Путь | Авторизация | Описание |
|-------|------|:-----------:|---------|
| GET | `/public/connection` | — | Обнаружение id/relay-сервера и публичного ключа |
| POST | `/api/login` | — | Логин, получение Bearer-токена |
| GET | `/api/login-options` | — | Список включённых OIDC-провайдеров |
| POST | `/api/logout` | ✓ | Выход, инвалидация токена |
| POST | `/api/oidc/auth` | — | Начать вход через Яндекс |
| GET | `/api/oidc/auth-query` | — | Опрос результата входа через Яндекс |
| POST | `/api/currentUser` | ✓ | Текущий пользователь + `entitlements` (тарифные фичи-флаги) |
| GET | `/api/users` | ✓ (admin) | Список пользователей тенанта |
| GET | `/api/peers` | ✓ | Список устройств пользователя |
| POST | `/api/heartbeat` | — | Heartbeat устройства (RustDesk bg) |
| POST | `/api/sysinfo` | — | Системная информация (RustDesk bg) |
| POST | `/api/audit/conn` | — | Событие сессии (RustDesk bg) |
| POST | `/api/audit/file` | — | Событие передачи файла (RustDesk bg) |
| GET | `/api/ab` | ✓ | Адресная книга целиком (legacy) |
| POST | `/api/ab` | ✓ | Полная замена AB (legacy) |
| POST | `/api/ab/personal` | ✓ | Получить GUID личной AB |
| POST | `/api/ab/settings` | ✓ | Настройки AB |
| POST | `/api/ab/shared/profiles` | ✓ | Общие AB тенанта |
| POST | `/api/ab/peers` | ✓ | Список устройств в AB |
| POST | `/api/ab/peer/add/{guid}` | ✓ | Добавить устройство |
| PUT | `/api/ab/peer/update/{guid}` | ✓ | Обновить устройство |
| DELETE | `/api/ab/peer/{guid}` | ✓ | Удалить устройства |
| POST | `/api/ab/tags/{guid}` | ✓ | Список тегов |
| POST | `/api/ab/tag/add/{guid}` | ✓ | Добавить тег |
| PUT | `/api/ab/tag/update/{guid}` | ✓ | Обновить цвет тега |
| PUT | `/api/ab/tag/rename/{guid}` | ✓ | Переименовать тег |
| DELETE | `/api/ab/tag/{guid}` | ✓ | Удалить теги |

---

*Документация актуальна для EvertyDesk API Server v1.x.*
