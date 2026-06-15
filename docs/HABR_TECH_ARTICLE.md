# EvertyDesk Lite изнутри: RustDesk-совместимый клиент, который пришлось заставить работать там, где «просто поставьте драйвер» не вариант

Это не рекламная статья про красивое окно удаленного доступа. Это разбор того, что обычно ломается, когда пытаешься сделать нативный remote desktop-клиент для реальной поддержки: корпоративный Linux, старые видеодрайверы, странные виртуалки, Astra Linux, РЕД ОС, Windows без нужных библиотек, адресная книга через API, терминал, подтверждение входящего подключения и кодеки.

Проект называется EvertyDesk Lite. Он написан на Rust, интерфейс построен на egui/eframe, а поведение ориентировано на RustDesk-совместимый сценарий: ID server, relay server, подключение по числовому ID, пароль или подтверждение на стороне удаленной машины.

Главная идея была грубой и практичной: клиент должен открыться не только на свежем ноутбуке разработчика, но и на машине, которую принесли из реального контура. Даже если там старый Linux, нестабильный OpenGL, нет WebView, нет желания тащить Electron, а администратору нужно подключиться прямо сейчас.

## Проблема 1. «Нативный UI» внезапно не запускается

Первая наивная версия любого desktop-клиента обычно выглядит так:

```rust
fn main() {
    eframe::run_native(
        "EvertyDesk Lite",
        eframe::NativeOptions::default(),
        Box::new(|_| Box::new(EvertyDeskApp::new())),
    )
    .unwrap();
}
```

На обычной Windows-машине это выглядит нормально. На части Linux-систем тоже. А потом приложение попадает в среду, где:

- OpenGL старый или сломан;
- Mesa стоит, но работает не так, как ожидается;
- сессия X11 есть, но драйвер ведет себя странно;
- Wayland недоступен;
- WebView ставить нельзя;
- пользователь не имеет прав доставить пакеты;
- машина находится в закрытом контуре.

В такой среде фраза «обновите драйвер» звучит красиво только в тикете. В реальности она означает «мы не можем помочь пользователю».

Поэтому в EvertyDesk Lite интерфейс имеет отдельный software backend. Это не «ускорение», а аварийный режим, который рисует egui на CPU и отдает пиксели в обычное окно через `minifb`.

Упрощенный вариант:

```rust
pub fn run_software_ui() -> Result<(), String> {
    std::env::set_var("EVERTYDESK_EGUI_SOFTWARE", "1");

    let mut window = Window::new(
        "EvertyDesk Lite",
        1100,
        760,
        WindowOptions {
            resize: true,
            scale_mode: ScaleMode::UpperLeft,
            ..WindowOptions::default()
        },
    )
    .map_err(|err| format!("open CPU egui window failed: {err}"))?;

    window.set_target_fps(60);

    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(1.0);
    configure_software_fonts(&ctx);
    configure_style(&ctx);

    let mut app = EvertyDeskApp::new();
    let mut painter = SoftwarePainter::default();
    let mut pixels = vec![0_u32; 1100 * 760];

    while window.is_open() && !window.is_key_down(MiniKey::Escape) {
        let (width, height) = window.get_size();
        pixels.resize(width * height, 0);

        let raw_input = collect_input(&window, width, height);

        let output = ctx.run(raw_input, |ctx| {
            app.update_egui(ctx);
        });

        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        painter.apply_textures(output.textures_delta);

        pixels.fill(0x14181c);
        painter.paint(&mut pixels, width, height, &primitives);

        window
            .update_with_buffer(&pixels, width, height)
            .map_err(|err| format!("CPU egui window update failed: {err}"))?;
    }

    app.shutdown();
    Ok(())
}
```

В обычном режиме используется `eframe` с `wgpu`/`glow`. В аварийном режиме используется CPU. Да, это не так модно. Зато на тестах с Astra Linux и РЕД ОС такой подход оказался гораздо полезнее, чем очередная попытка угадать правильный графический backend.

Запуск:

```bash
EVERTYDESK_RENDERER=software ./evertydesk-lite
```

Именно это я имел в виду под «запустить даже на марсианском корабле». Если у вас есть хоть какой-то X11 и окно можно открыть, шанс уже есть.

## Проблема 2. Кириллица в fallback UI

Когда переходишь на software render, быстро выясняется еще одна неприятная вещь: текстовый ввод и шрифты перестают быть «просто деталями». Русский интерфейс, русские заметки в адресной книге, русские команды в терминале, русские сообщения ошибок — все это должно отображаться нормально.

Решение скучное, но рабочее: выставить UTF-8 locale и явно найти системный шрифт с кириллицей.

```rust
fn configure_locale_for_text_input() {
    #[cfg(unix)]
    unsafe {
        for locale in [
            b"\0".as_slice(),
            b"C.UTF-8\0",
            b"ru_RU.UTF-8\0",
            b"en_US.UTF-8\0",
        ] {
            let active = libc::setlocale(libc::LC_CTYPE, locale.as_ptr().cast());
            if is_utf8_locale(active) {
                break;
            }
        }
    }
}
```

```rust
fn load_cyrillic_font() -> Option<(String, Vec<u8>)> {
    const FONT_PATHS: &[(&str, &str)] = &[
        ("Noto Sans", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"),
        ("DejaVu Sans", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
        ("Liberation Sans", "/usr/share/fonts/liberation-fonts/LiberationSans-Regular.ttf"),
        ("Segoe UI", "C:\\Windows\\Fonts\\segoeui.ttf"),
        ("Arial", "C:\\Windows\\Fonts\\arial.ttf"),
    ];

    for (name, path) in FONT_PATHS {
        if let Ok(data) = std::fs::read(path) {
            if !data.is_empty() {
                return Some((format!("system-{name}"), data));
            }
        }
    }
    None
}
```

Это выглядит примитивно, но это ровно тот слой, который отличает «демо запустилось» от «приложением можно пользоваться на рабочей машине».

## Проблема 3. Кодеки: H.264 есть, VP9 есть, но не везде одинаково

Для удаленного рабочего стола важно не только показать картинку, но и не умереть на сборке. В проекте сейчас используется практичный набор:

- H.264 через `openh264`;
- VP9 на Windows через Media Foundation;
- VP9 на Linux через `libvpx`, если окружение позволяет;
- fallback на кадры изображения, если live video path недоступен.

Фичи в `Cargo.toml` выглядят так:

```toml
[features]
default = ["live-h264", "live-vp9-mf"]
live-h264 = ["dep:openh264"]

# VP9 через сборку libvpx из исходников.
# Подходит не всем старым окружениям.
live-vpx = ["dep:shiguredo_libvpx", "shiguredo_libvpx/source-build"]

# VP9 через системный libvpx: apt install libvpx-dev
live-vpx-system = []

# VP9 через Windows Media Foundation
live-vp9-mf = ["dep:windows"]
```

Наивное решение — включить все и радоваться. Практическое решение — дать сборщику выбор.

```bash
# Windows: H.264 + VP9 через Media Foundation
cargo build --release --features live-h264,live-vp9-mf

# Linux: H.264 + системный libvpx
cargo build --release --no-default-features --features live-h264,live-vpx-system

# Минимальный режим без live video path
cargo build --release --no-default-features
```

В коде режим должен быть видимым, а не магическим:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveVideoMode {
    ScreenshotOnly,
    H264,
    H264Vpx,
}

impl LiveVideoMode {
    pub fn current() -> Self {
        if cfg!(feature = "live-h264") && cfg!(feature = "live-vpx") {
            Self::H264Vpx
        } else if cfg!(feature = "live-h264") {
            Self::H264
        } else {
            Self::ScreenshotOnly
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ScreenshotOnly => "PNG only",
            Self::H264 => "H264",
            Self::H264Vpx => "H264 + VP8/VP9",
        }
    }
}
```

Здесь есть важный технический вывод: remote desktop-клиент не должен зависеть от одного идеального media pipeline. В реальном мире pipeline ломается. Клиент должен деградировать, но оставаться полезным.

## Проблема 4. RustDesk-compatible API и 401, который не объясняет ничего

Адресная книга кажется простой, пока не подключаешь реальный API. Нужна авторизация, токен, персональная адресная книга, список peers, добавление, обновление, удаление.

Типичный путь:

1. `POST /api/login`
2. получить `access_token`
3. `POST /api/ab/personal`
4. получить `guid`
5. `POST /api/ab/peers?ab=<guid>&pageSize=30&current=1`

Главная ошибка, которую легко сделать: считать, что логин возвращает только токен. На практике API может вернуть сообщение, другой тип ответа, требование дополнительного шага или ошибку с неидеальным кодом.

Код авторизации:

```rust
pub(crate) fn login(
    api_url: &str,
    username: &str,
    password: &str,
    rustdesk_id: &str,
    uuid: &str,
) -> Result<String, String> {
    let json = api_public_send(
        "POST",
        api_url,
        "/api/login",
        serde_json::json!({
            "username": username,
            "password": password,
            "id": normalize_remote_id(rustdesk_id),
            "uuid": uuid,
            "autoLogin": true,
            "type": "account",
            "deviceInfo": {
                "os": platform(),
                "type": "PC",
                "name": local_hostname(),
            }
        }),
    )?;

    check_json_error(&json)?;

    extract_string_field(&json, "access_token")
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| {
            extract_string_field(&json, "type")
                .map(|kind| format!("Login requires extra step: {kind}"))
                .unwrap_or_else(|| "API did not return access_token".to_owned())
        })
}
```

Список контактов:

```rust
pub(crate) fn peers(api_url: &str, token: &str, guid: &str) -> Result<Vec<ContactEntry>, String> {
    let mut contacts = Vec::new();
    let mut current = 1usize;

    loop {
        let current_s = current.to_string();
        let json = api_send(
            "POST",
            api_url,
            token,
            "/api/ab/peers",
            serde_json::json!({}),
            &[("ab", guid), ("pageSize", "30"), ("current", &current_s)],
        )?;

        check_json_error(&json)?;

        let data = json
            .get("data")
            .and_then(Value::as_array)
            .or_else(|| json.as_array())
            .ok_or_else(|| "API did not return peers list".to_owned())?;

        for peer in data {
            if let Some(remote_id) = extract_string_field(peer, "id") {
                contacts.push(ContactEntry {
                    name: extract_string_field(peer, "alias").unwrap_or_default(),
                    remote_id: normalize_remote_id(&remote_id),
                    note: extract_string_field(peer, "hostname").unwrap_or_default(),
                    machine_id: String::new(),
                    os: extract_string_field(peer, "platform").unwrap_or_default(),
                    last_seen: String::new(),
                    online: false,
                });
            }
        }

        let total = json
            .get("total")
            .and_then(Value::as_u64)
            .unwrap_or(contacts.len() as u64);

        if data.len() < 30 || (current as u64) * 30 >= total {
            break;
        }

        current += 1;
    }

    Ok(contacts)
}
```

Тот самый неприятный случай `401` решается не «попробуйте еще раз», а нормальной диагностикой:

```rust
fn response_json(result: Result<ureq::Response, ureq::Error>) -> Result<Value, String> {
    match result {
        Ok(response) => response
            .into_string()
            .map_err(|err| format!("API response error: {err}"))
            .and_then(|text| {
                if text.trim().is_empty() {
                    Ok(serde_json::json!({ "ok": true }))
                } else {
                    serde_json::from_str::<Value>(&text)
                        .map_err(|err| format!("API JSON error: {err}"))
                }
            }),
        Err(ureq::Error::Status(code, response)) => {
            let text = response.into_string().unwrap_or_default();
            Err(format!("API HTTP {code}: {text}"))
        }
        Err(err) => Err(format!("API request failed: {err}")),
    }
}
```

После этого оператор видит не абстрактное «не работает», а конкретное:

```text
API HTTP 401: {"code":500,"data":null,"message":"Unauthorized"}
```

И уже понятно, что проблема не в UI, а в токене, логине, пароле, endpoint или схеме авторизации.

## Проблема 5. Подключение без пароля должно спрашивать подтверждение

Remote desktop без подтверждения — это не фича, а инцидент. Если оператор не ввел пароль, удаленная сторона должна явно принять подключение.

Архитектурно это выглядит как отдельное событие host-части:

```rust
pub enum HostEvent {
    ApprovalRequested {
        session_id: String,
        peer_id: String,
    },
    ClientConnected {
        session_id: String,
    },
    ClientDisconnected {
        session_id: String,
    },
}
```

На стороне UI это не должно теряться в логах. Нужен модальный запрос:

```rust
fn show_approval_window(&mut self, ctx: &egui::Context) {
    for request in self.pending_approvals.clone() {
        egui::Window::new("Входящее подключение")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("ID: {}", request.peer_id));

                ui.horizontal(|ui| {
                    if ui.button("Разрешить").clicked() {
                        self.host.approve(&request.session_id);
                    }

                    if ui.button("Отклонить").clicked() {
                        self.host.reject(&request.session_id);
                    }
                });
            });
    }
}
```

Смысл здесь не в кнопках. Смысл в том, что password flow и approval flow — разные сценарии, и их нельзя смешивать.

## Проблема 6. Терминал нужен не меньше экрана

Когда поддержка подключается к машине, экран часто нужен только для первичного контекста. Дальше начинаются команды:

```bash
systemctl status ...
journalctl -u ...
ip a
df -h
cat /etc/os-release
```

Если в клиенте есть только «картинка экрана», администратор вынужден открывать терминал руками, передавать ввод через remote input, ловить раскладку, копирование, спецсимволы. Это раздражает и съедает время.

Поэтому в EvertyDesk Lite добавлен режим консоли: отдельная кнопка рядом с подключением, отдельный канал сообщений, отдельное окно вывода. Концептуально это выглядит так:

```rust
pub enum ShellMessage {
    Open {
        session_id: String,
    },
    Input {
        session_id: String,
        data: Vec<u8>,
    },
    Output {
        session_id: String,
        data: Vec<u8>,
    },
    Close {
        session_id: String,
    },
}
```

Ключевой момент: терминал не должен притворяться экраном. Это другой инструмент. Для него нужны свои буферы, история, вставка, обработка вывода и понятные ограничения безопасности.

## Проблема 7. AI в терминале: полезно, если не давать ему рулить вслепую

AI-фича в таком клиенте не должна быть «магической кнопкой починить сервер». Это опасно. Правильный вариант — ассистент, который анализирует вывод терминала и предлагает следующую команду или объясняет ошибку.

Поддерживаются разные провайдеры:

- локальная Ollama;
- OpenAI-compatible endpoint;
- YandexGPT.

Промпт строится из цели оператора и хвоста терминального вывода. Контекст ограничен, чтобы не отправлять бесконечную историю.

```rust
const MAX_TERMINAL_CONTEXT_CHARS: usize = 14_000;

fn terminal_prompt(transcript: &str, goal: &str) -> String {
    let transcript = tail_text(transcript, MAX_TERMINAL_CONTEXT_CHARS);

    format!(
        "Цель оператора:\n{goal}\n\n\
         Последний вывод удаленного терминала:\n```text\n{transcript}\n```\n\n\
         Ответь структурно: диагноз, команда для вставки, \
         что проверить после выполнения."
    )
}

fn tail_text(text: &str, max_chars: usize) -> String {
    let chars = text.chars().count();
    if chars <= max_chars {
        return text.to_owned();
    }
    text.chars().skip(chars - max_chars).collect()
}
```

Вызов OpenAI-compatible API:

```rust
fn call_openai(config: &LlmConfig, system: &str, user: &str) -> Result<String, String> {
    let api_key = required(&config.openai_api_key, "OpenAI API key")?;
    let model = required(&config.openai_model, "OpenAI model")?;
    let url = required(&config.openai_base_url, "OpenAI endpoint")?;

    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "temperature": safe_temperature(config.temperature),
        "max_tokens": safe_max_tokens(config.max_tokens)
    });

    let value = post_json(
        ureq::post(url)
            .timeout(Duration::from_secs(60))
            .set("Authorization", &format!("Bearer {api_key}"))
            .set("Content-Type", "application/json"),
        body,
    )?;

    value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "OpenAI: не найден choices[0].message.content".to_owned())
}
```

Самое важное ограничение: AI не должен автоматически выполнять команды. Он предлагает. Оператор вставляет и запускает сам. В удаленном администрировании это не бюрократия, а нормальная граница ответственности.

## Проблема 8. Служба должна работать до входа пользователя

Клиент удаленного доступа как обычное окно — это хорошо для исходящих подключений. Но для входящего доступа нужен host/service mode: машина должна быть доступна после старта ОС, до логина пользователя и без ручного запуска окна.

На Windows это служба. На Linux — systemd unit. Минимальный вариант unit-файла:

```ini
[Unit]
Description=EvertyDesk Lite host service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/evertydesk-lite --host
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
```

Команды:

```bash
sudo install -m 0755 evertydesk-lite /usr/local/bin/evertydesk-lite
sudo install -m 0644 evertydesk-lite.service /etc/systemd/system/evertydesk-lite.service
sudo systemctl daemon-reload
sudo systemctl enable --now evertydesk-lite.service
```

Тут есть отдельный пласт проблем: доступ к экрану, X11/Wayland, права на ввод, сессии пользователей. Поэтому service mode нельзя рассматривать как одну галочку. Это отдельный режим продукта.

## Проблема 9. Сборка под Linux не должна зависеть от памяти автора

Если проект требует «ну я у себя поставил пару пакетов, не помню каких», он не готов к Linux.

Минимальная диагностика окружения должна быть скриптом:

```bash
#!/usr/bin/env bash
set -euo pipefail

echo "OS:"
cat /etc/os-release || true

echo "Graphics:"
command -v glxinfo >/dev/null && glxinfo -B || true

echo "Rust:"
rustc --version
cargo --version

echo "Libraries:"
pkg-config --modversion x11 || true
pkg-config --modversion vpx || true
```

А сборка должна иметь явные варианты:

```bash
# Без VP9, если libvpx недоступен
cargo build --release --no-default-features --features live-h264

# С системным VP9
cargo build --release --no-default-features --features live-h264,live-vpx-system

# С аварийным software UI при запуске
EVERTYDESK_RENDERER=software ./target/release/evertydesk-lite
```

Это особенно важно для Astra Linux и РЕД ОС, где окружение может быть старше, чем ожидал автор библиотеки.

## Что получилось

Сейчас EvertyDesk Lite — это не «полный клон всего на свете», а компактный RustDesk-совместимый клиент с упором на практическую поддержку:

- подключение по ID;
- пароль или подтверждение входящего подключения;
- адресная книга через RustDesk-compatible API;
- история подключений и заметки;
- терминал;
- AI-подсказки для терминала;
- H.264 и VP9 там, где они доступны;
- fallback без live video path;
- egui-интерфейс;
- software render для проблемных Linux-графических окружений;
- service/host mode как отдельное направление.

Самый важный технический урок: remote desktop-клиент нельзя проектировать как приложение для идеальной машины. Он почти всегда нужен именно тогда, когда машина не идеальна.

Поэтому здесь Rust, потому что нужен контроль и предсказуемость. Здесь egui, потому что не нужен встроенный браузер. Здесь software render, потому что OpenGL может подвести. Здесь несколько video path, потому что кодеки не одинаковы на всех ОС. Здесь терминал и AI, потому что поддержка работает не только глазами, но и командами.

И да, если завтра придется подключаться с условного марсианского корабля, где есть только старый X11 и странные драйверы, я хочу, чтобы шанс все равно был.
