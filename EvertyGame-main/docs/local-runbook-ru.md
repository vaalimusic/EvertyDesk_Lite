# Локальный runbook Everty

## Что это такое

Для обычного локального теста теперь есть простой сценарий:

1. Поднять `control-plane`
2. На игровом ПК открыть `receiver-native` в режиме `Send`
3. Войти быстрым демо-входом `admin/admin` или `test/test`
4. На телефоне открыть Android-приложение
5. Войти тем же быстрым демо-входом
6. Увидеть зарегистрированный ПК в списке и нажать `Подключиться`

Для первого подключения больше не нужен:

- `/admin`
- marketplace offer
- ручной ввод сложных route / relay / probe параметров

Клиентский простой режим теперь берет список ПК из обычного `GET /api/hosts`.

## Что должно быть установлено

На машине должны быть установлены:

- `PowerShell`
- `.NET 8 SDK`
- `Git`
- `Docker Desktop` и `docker compose`, если запуск нужен через контейнеры

Проверка:

```powershell
dotnet --version
git --version
docker version
docker compose version
```

Если `docker version` падает, значит Docker Desktop не запущен.

## Откуда запускать команды

Все команды выполнять из корня репозитория:

```powershell
cd C:\Users\VAALI\AndroidStudioProjects\EvertyGame
```

## Демо-учетки

В локальном/dev режиме `control-plane` автоматически сидит две demo-учетки:

- `admin / admin`
- `test / test`

Это управляется флагом:

```text
EVERTY_CONTROL_PLANE_DEMO_AUTH_ENABLED=true
```

Флаг уже включен по умолчанию в:

- `deploy/control-plane.env.example`
- `deploy/docker-compose.env.example`
- `control-plane/Dockerfile`
- `docker-compose.yml`

## Дефолтный адрес сервера

Для локального LAN-теста основной адрес:

```text
http://192.168.0.5:5180
```

Он выставлен как дефолт в:

- desktop `receiver-native`
- Android `PC Receiver`

## Самый простой сценарий: игровой ПК + телефон

### 1. Поднять backend

Без Docker:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\publish-platform.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\start-platform-local.ps1
```

Проверка:

- `http://192.168.0.5:5180/api/ready`

### 2. Открыть `receiver-native` на игровом ПК

Что сделать:

- выбрать режим `Send`
- проверить, что в поле сервера стоит `http://192.168.0.5:5180`
- нажать быстрый вход `Войти как admin` или `Войти как test`

Ожидаемое поведение:

- появится понятный статус входа
- ПК автоматически зарегистрируется как host
- в статусе будет смысловой текст вроде:
  - `Этот ПК виден другим устройствам`
  - `Жди подключения с телефона или другого клиента`

Для появления ПК на телефоне **не нужно** нажимать `Start Sending`.

`Start Sending` остается только как manual/advanced путь.

### 3. Открыть Android-приложение на телефоне

Что сделать:

- проверить адрес сервера `http://192.168.0.5:5180`
- нажать `Войти как admin` или `Войти как test`
- дождаться автозагрузки списка ПК
- выбрать нужный ПК
- нажать `Подключиться`

Ожидаемое поведение:

- после входа приложение само загружает список компьютеров
- если ПК зарегистрирован, он появится без operator console и без marketplace offer

## Что делать, если список ПК пустой

Теперь приложение должно показывать понятные причины, а не просто пустой список.

Проверять по порядку:

1. Открывается ли `http://192.168.0.5:5180/api/ready`
2. Запущен ли `receiver-native` на игровом ПК
3. Выбран ли на ПК режим `Send`
4. Выполнен ли вход на ПК
5. Есть ли на ПК статус, что этот компьютер виден другим устройствам

Если backend жив, а ПК вошел и зарегистрировался, телефон должен увидеть host через обычный список `/api/hosts`.

## Запуск через Docker

Если нужен Docker path:

```powershell
docker compose --env-file deploy/docker-compose.env.example up --build
```

Проверка:

- `http://127.0.0.1:5180/api/ready`

Остановить:

```powershell
docker compose --env-file deploy/docker-compose.env.example down
```

## Операторская панель

`/admin` больше не нужен для первого пользовательского сценария.

Он нужен только для operator/admin задач:

- summary
- stop session
- relay/host availability
- marketplace
- billing reconciliation

Открыть:

- `http://127.0.0.1:5180/admin`

Operator key по env-template:

- `replace-with-long-random-operator-key`

## Быстрая проверка проекта

Если нужно быстро проверить текущий product-ready контур без медленного payment-provider smoke:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-product-readiness.ps1 -SkipPaymentProvider
```

Скрипт делает:

- сборку `control-plane`
- сборку `relay-node`
- проверку `docker compose config`
- readiness smoke
- persistence smoke
- security smoke
- admin smoke
- dashboard smoke
- CLI smoke
- route-policy smoke
- release hygiene audit

Если все хорошо, в конце будет статус `ok`.

## Дополнительные проверки

Проверка release hygiene:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\audit-release-hygiene.ps1
```

Проверка commit scope:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\audit-commit-scope.ps1
```

## Проверка published artifacts

Если нужно проверить именно опубликованные артефакты:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\publish-platform.ps1 -Version 0.1.0-product -Channel local-product
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-published-platform.ps1
```

Артефакты находятся в:

- `artifacts/platform/control-plane`
- `artifacts/platform/relay-node`
- `artifacts/platform/publish-manifest.json`

## Проверка payment-provider сценария

Happy-path smoke:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File control-plane\smoke-payment-provider.ps1
```

Retry/failure path:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File control-plane\smoke-payment-provider.ps1 -BaseUrl http://127.0.0.1:5206 -ProviderPort 5207 -FailAction capture -FailOnce
```

## Где смотреть документацию

Основные файлы:

- `docs/deployment.md`
- `docs/release-readiness.md`
- `docs/release-candidate-handoff.md`

## Частые проблемы

`docker compose ... up --build` падает с ошибкой про `dockerDesktopLinuxEngine`:

- Docker Desktop не запущен
- нужно запустить Docker Desktop и дождаться `Engine running`

`dotnet build` не работает:

- не установлен `.NET 8 SDK`
- проверить `dotnet --version`

PowerShell-скрипт не стартует:

- запускать с `-ExecutionPolicy Bypass`

`/admin` не открывается:

- сначала проверить `http://127.0.0.1:5180/api/ready`
- если `ready` не отвечает, backend не поднялся

Телефон не видит ПК:

- backend недоступен
- на игровом ПК не открыт `receiver-native`
- на игровом ПК не выбран режим `Send`
- на игровом ПК не выполнен вход
- ПК еще не зарегистрировался в `control-plane`

## Самый практичный сценарий для этой машины

Если нужен просто рабочий локальный запуск без Docker:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\publish-platform.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\start-platform-local.ps1
```

Потом:

1. открыть `receiver-native` на игровом ПК
2. войти `admin/admin`
3. открыть Android-приложение
4. войти `admin/admin`
5. выбрать ПК
6. нажать `Подключиться`

Остановить локальные сервисы:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\stop-platform-local.ps1
```
