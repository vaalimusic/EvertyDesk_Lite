# EVRT: почему быстрый remote desktop начинается не с кодека, а с правильного транспорта

Когда мы говорим про удаленный рабочий стол, первое, о чем обычно спорят, это кодек.
H.264, H.265, VP9, AV1, NVENC, Media Foundation, VideoToolbox, OpenH264. Все правильно:
кодек важен. Но если смотреть на систему глазами задержки, кодек - это только один этаж.

Настоящая проблема начинается ниже:

- как быстро кадр попадает из capture в encoder;
- как он проходит сеть;
- что делать, если один UDP-пакет потерялся;
- как не копить очередь из старых кадров;
- как клиент сообщает хосту, что он уже не успевает;
- как не сломать fallback через RustDesk-compatible relay;
- как оставить управление, shell, авторизацию и безопасность в надежном канале;
- как сделать так, чтобы система деградировала, но не превращалась в черный экран.

Для этого в EvertyDesk Lite появился EVRT.

EVRT, Everty Real-Time, - это отдельный low-latency слой поверх RustDesk-compatible сценария.
RustDesk-совместимая часть остается для авторизации, peer discovery, relay fallback и control path.
А EVRT берет на себя то, что в интерактивном сценарии болит сильнее всего: доставку видео и аудио с минимальной задержкой.

Если формулировать совсем коротко: EVRT - это игровой транспорт внутри remote desktop клиента.

И вот здесь начинается самое интересное. Код Артура гениален не потому, что в нем есть магические названия.
Он гениален в инженерном смысле: он не пытается выиграть за счет одной большой идеи. Он выигрывает за счет набора маленьких, очень точных решений, которые вместе дают поведение, похожее на игровой стриминг, а не на "пересылку скриншотов".

## Почему обычного RustDesk-compatible канала мало

RustDesk-compatible путь хорош тем, что он практичный:

- есть ID server;
- есть relay server;
- есть авторизация;
- есть NAT traversal;
- есть fallback, если прямое соединение не получилось;
- есть совместимость с существующей моделью подключения.

Но relay-путь не всегда подходит для игрового управления. У него другая философия: доставить надежно, пройти через сложную сеть, не требовать от пользователя ручной настройки. Для поддержки это нормально. Для мыши, игры, тачпада и динамичной картинки это уже больно.

Поэтому EVRT не ломает старую систему. Он делает правильнее: оставляет RustDesk-compatible слой как control plane, а видео переводит на отдельный direct UDP fast path.

Схема получается такая:

```text
RustDesk-compatible layer:
  login / auth / peer discovery / relay fallback / control / shell

EVRT layer:
  direct UDP / video frames / audio / feedback / latency metrics
```

Это первое сильное решение: не переписывать весь мир, а добавить быстрый путь там, где он действительно нужен.

## Протокол должен быть маленьким

У EVRT бинарный заголовок на 24 байта. Это намеренно скучно и правильно.

В `src/evrt.rs` есть базовые константы:

```rust
pub const MAGIC: u32 = 0x4556_5254; // "EVRT"
pub const VERSION: u8 = 3;
pub const HEADER_SIZE: usize = 24;
pub const MAX_PACKET_SIZE: usize = 1200;
pub const MAX_PAYLOAD_SIZE: usize = MAX_PACKET_SIZE - HEADER_SIZE;

pub const TYPE_SESSION_CONFIG: u8 = 1;
pub const TYPE_CODEC_CONFIG: u8 = 2;
pub const TYPE_VIDEO_FRAME: u8 = 3;
pub const TYPE_CONTROL: u8 = 4;
pub const TYPE_AUDIO_CONFIG: u8 = 5;
pub const TYPE_AUDIO_FRAME: u8 = 6;
pub const TYPE_ENHANCEMENT_CONFIG: u8 = 7;
pub const TYPE_ENHANCEMENT_FRAME: u8 = 8;
pub const TYPE_ROI_METADATA: u8 = 9;
```

Здесь важны две вещи.

Первая: `MAX_PACKET_SIZE = 1200`. Это не случайное число. UDP-пакеты нельзя просто делать "побольше, чтобы быстрее". Большие датаграммы чаще фрагментируются, а фрагментация в real-time видео - зло. Один потерянный фрагмент убивает весь пакет. Поэтому EVRT держит packet size в MTU-safe зоне.

Вторая: протокол сразу разделяет типы данных:

- session config;
- codec config;
- video frame;
- control;
- audio config;
- audio frame;
- enhancement frame;
- ROI metadata.

Это не "потом как-нибудь разберем". Это фундамент для расширения. Можно добавлять аудио, enhancement stream, ROI и feedback, не ломая базовую доставку видео.

Сборка пакета намеренно прямолинейная:

```rust
pub fn build_packet(
    packet_type: u8,
    flags: u16,
    frame_id: u32,
    packet_index: u16,
    packet_count: u16,
    presentation_time_us: u64,
    payload: &[u8],
) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(HEADER_SIZE + payload.len());
    pkt.extend_from_slice(&MAGIC.to_be_bytes());
    pkt.push(VERSION);
    pkt.push(packet_type);
    pkt.extend_from_slice(&flags.to_be_bytes());
    pkt.extend_from_slice(&frame_id.to_be_bytes());
    pkt.extend_from_slice(&packet_index.to_be_bytes());
    pkt.extend_from_slice(&packet_count.to_be_bytes());
    pkt.extend_from_slice(&presentation_time_us.to_be_bytes());
    pkt.extend_from_slice(payload);
    pkt
}
```

Это хороший код не потому, что он выглядит сложно. Наоборот. Он хорош тем, что здесь нечему неожиданно сломаться. Заголовок фиксированный, порядок байт явный, версия есть, magic есть, packet type есть.

Для real-time протокола это важнее красоты.

## Кадр нужно резать на пакеты

Видео-кадр почти всегда больше одного UDP-пакета. Значит, его нужно разбить, пронумеровать, отправить и потом собрать на клиенте.

В EVRT это выглядит так:

```rust
pub fn packetize_video_frame(
    frame_id: u32,
    presentation_time_us: u64,
    is_key_frame: bool,
    payload: &[u8],
) -> Vec<Vec<u8>> {
    let flags = if is_key_frame { FLAG_KEY_FRAME } else { 0 };
    packetize(
        TYPE_VIDEO_FRAME,
        flags,
        frame_id,
        presentation_time_us,
        payload,
    )
}
```

Снаружи это маленькая функция. Но архитектурно она задает правильную модель:

- у каждого кадра есть `frame_id`;
- каждый UDP-пакет знает свой `packet_index`;
- каждый UDP-пакет знает `packet_count`;
- keyframe помечается флагом;
- timestamp идет вместе с кадром.

Это позволяет клиенту не гадать, что происходит. Он видит: пришел кусок такого-то кадра, это пакет 3 из 14, кадр ключевой или нет, timestamp такой-то.

Для игр и интерактивного desktop это критично. Если потерялась часть P-frame, нельзя продолжать делать вид, что все хорошо. Нужно запросить keyframe и синхронизироваться.

## Главная идея: один capture, один encoder, два транспорта

Самое важное в EVRT не UDP. Самое важное - как UDP встроен в video pipeline.

В `src/video_pipeline.rs` есть единая структура кадра:

```rust
#[derive(Clone)]
pub struct EncodedFrame {
    pub bytes: Arc<Vec<u8>>,
    pub is_idr: bool,
    pub frame_id: u32,
    pub pts_us: u64,
    pub display: i32,
    pub sps_pps: Option<Arc<Vec<u8>>>,
    pub width: u32,
    pub height: u32,
    pub codec: &'static str,
    pub roi: crate::evrt::RoiRect,
}
```

Обратите внимание на `Arc<Vec<u8>>`. Это не мелочь. Кадр может уйти в TCP fallback и в EVRT sender без полного копирования encoded payload. Для high FPS и больших разрешений такие решения быстро становятся видимыми.

Пайплайн делает так:

```text
CaptureThread -> EncodeThread -> Dispatcher
                                  |-> TCP relay sender
                                  |-> EVRT UDP sender
```

То есть EVRT не запускает второй capture и второй encoder. Это было бы плохо:

- двойная нагрузка на GPU/CPU;
- разные кадры в разных каналах;
- гонки при переключении;
- сложная синхронизация;
- больше latency.

Вместо этого есть один источник правды: encoded frame.

Дальше dispatcher принимает решение:

```rust
if evrt_on {
    match evrt_tx.try_send(frame.clone()) {
        Ok(()) => {}
        Err(mpsc::TrySendError::Full(_)) => {
            force_recovery_key = true;
            apply_tcp_backpressure(&bitrate_scale_milli);
        }
        Err(mpsc::TrySendError::Disconnected(_)) => break,
    }

    if is_idr {
        match send_tcp_video_frame(&tcp_tx, &stop, frame) {
            TcpVideoSend::Sent => force_recovery_key = false,
            TcpVideoSend::Dropped => {
                force_recovery_key = true;
                apply_tcp_backpressure(&bitrate_scale_milli);
            }
            TcpVideoSend::Disconnected => break,
        }
    }
} else {
    match send_tcp_video_frame(&tcp_tx, &stop, frame) {
        TcpVideoSend::Sent => {
            if is_idr {
                force_recovery_key = false;
            }
        }
        TcpVideoSend::Dropped => {
            force_recovery_key = true;
            apply_tcp_backpressure(&bitrate_scale_milli);
        }
        TcpVideoSend::Disconnected => break,
    }
}
```

Это один из самых сильных фрагментов архитектуры.

Когда EVRT активен, все кадры идут в UDP. Но IDR дополнительно уходит в TCP. Зачем?

Потому что IDR - это точка синхронизации. Если UDP потерял что-то важное, клиент может восстановиться от ключевого кадра. А TCP все еще остается надежным резервным каналом.

Когда EVRT не активен, все видео идет через TCP relay. То есть пользователь не получает "не поддерживается". Он получает fallback.

Это и есть инженерная зрелость: fast path не должен убивать slow path.

## Очередь кадров: брать свежее, а не честно показывать старое

В обычной очереди логика простая: что пришло первым, то и обработали первым.

Для remote desktop это часто неправильно.

Если пользователь двигает мышь, а у клиента накопилось 8 старых кадров, честный FIFO покажет ему прошлое. Картинка будет плавной, но управление будет ватным. Для игры это смерть.

В EVRT используется идея `LatestAccessUnitQueue`: если очередь забилась, лучше выбросить старое и взять свежий кадр.

В `src/frame_queue.rs` игровая конфигурация такая:

```rust
impl Default for FrameQueueConfig {
    fn default() -> Self {
        Self {
            max_queued_units: 2,
            max_queued_bytes: 512 * 1024,
            hard_reset_on_keyframe: false,
            prefer_latest: true,
            drop_current_on_wait: true,
            initial_jitter_delay: Duration::ZERO,
        }
    }
}
```

Ключевой параметр здесь - `prefer_latest: true`.

Внутри enqueue это превращается в простое правило:

```rust
if g.cfg.prefer_latest && !g.queue.is_empty() {
    g.clear_queue();
    g.push(bytes, is_key_frame, presentation_time_us);
    return;
}
```

И вот это выглядит почти слишком просто. Но именно такие простые правила часто дают real-time ощущение.

EVRT не пытается "доставить все кадры". Он пытается показать актуальное состояние удаленной машины.

Это разные задачи.

Для фильма мы хотим плавность. Для remote desktop и игры мы хотим свежесть.

Поэтому в коде есть два режима:

```rust
impl FrameQueueConfig {
    pub fn cinema() -> Self {
        Self {
            max_queued_units: 4,
            max_queued_bytes: 2 * 1024 * 1024,
            hard_reset_on_keyframe: false,
            prefer_latest: false,
            drop_current_on_wait: false,
            initial_jitter_delay: Duration::from_millis(16),
        }
    }
}
```

Игровой режим: минимальная задержка.

Cinema mode: чуть больше буфер, чуть больше плавность.

Это не "один ползунок качества". Это разные модели восприятия.

## Потерял кадр - проси IDR

Сжатое видео зависит от предыдущих кадров. Если потерять кусок потока и продолжать декодировать, можно получить артефакты, двоение, кашу или зависший кадр.

Поэтому клиент EVRT не героически терпит битый поток. Он просит keyframe.

В клиентском receive loop есть логика:

```rust
let drops_before = reassembler.dropped_frames();
if let Some((bytes, key, _delay_ms, pts)) = reassembler.on_packet(&pkt) {
    recv_assembled.fetch_add(1, Ordering::Relaxed);
    recv_queue.enqueue(bytes, key, pts);
}

let drops_after = reassembler.dropped_frames();
recv_reassembly_drops.store(drops_after, Ordering::Relaxed);

if drops_after > drops_before
    && last_loss_keyframe_request.elapsed() >= Duration::from_millis(250)
{
    let _ = recv_socket.send_to(&evrt::build_request_key_frame(), host_addr);
    recv_queue.wait_for_keyframe();
    last_loss_keyframe_request = Instant::now();
}
```

Смысл:

1. Reassembler видит потерю.
2. Клиент отправляет `RequestKeyFrame`.
3. Очередь переходит в режим ожидания keyframe.
4. P-frame больше не проигрываются, пока не придет IDR.

Это защищает от визуального мусора. Особенно важно для Linux-host сценариев, где можно увидеть "двоение" или плохую картинку, если поток восстановился не с правильной точки.

## Feedback loop: клиент не молчит

Большинство наивных remote desktop реализаций делают так:

```text
host: я отправляю 30 fps
client: я как-нибудь разберусь
```

EVRT делает иначе. Клиент постоянно говорит хосту, что происходит:

- сколько кадров в очереди;
- есть ли drops;
- какой arrival delta;
- какой decode delta;
- какой pressure;
- какой decode FPS.

Фрагмент из `src/evrt_client.rs`:

```rust
let pressure = compute_pressure(arr_delta, dec_delta, queued, new_drops, cinema);

let jitter_ms = jitter.update(pressure, arr_delta, queued, new_drops, cinema);
queue.set_jitter_delay(Duration::from_millis(jitter_ms as u64));

let fb = ReceiverFeedback {
    pressure,
    backlog_frames: queued,
    queue_drops: drops,
    decode_fps: fps_decoded,
    assembly_delay_ms: 0,
    arrival_delta_ms: arr_delta,
    decode_delta_ms: dec_delta,
    present_delta_ms: -1,
    pulse_estimate_ms: -1,
    input_estimate_ms: -1,
};

let pkt = evrt::build_receiver_feedback(&fb);
let _ = socket.send_to(&pkt, host_addr);
```

Это очень важная часть.

Клиент не просто получает поток. Клиент управляет потоком.

Если arrival delta растет, если decode не успевает, если очередь копится, если появились drops - это превращается в pressure. А pressure возвращается на хост.

## Adaptive relief: хост должен уметь отступить

На стороне хоста feedback превращается в снижение нагрузки.

В `src/evrt_session.rs`:

```rust
while let Ok(fb) = fb_rx.try_recv() {
    let cur_fps = target_fps.load(Ordering::Relaxed);
    if let Some(step) = relief.on_feedback(&fb, cur_fps) {
        let scale_milli = relief
            .apply_pending_milli()
            .unwrap_or_else(|| AdaptiveRelief::bitrate_scale_milli(step));

        bitrate_scale_milli.store(scale_milli, Ordering::Relaxed);

        evrt_log(
            &events,
            format!(
                "EVRT adaptive relief step={} scale={}pct pressure={}",
                relief.current_step(),
                scale_milli / 10,
                fb.pressure.as_str(),
            ),
        );
    }
}
```

Это не "поставим битрейт один раз в настройках". Это цикл адаптации.

Если клиент не справляется, хост должен уменьшить нагрузку. Не через диалоговое окно. Не через "пользователь сам пусть снизит FPS". Автоматически.

Потом bitrate попадает в encoder pipeline:

```rust
let relief_milli = bitrate_scale_milli
    .load(Ordering::Relaxed)
    .clamp(MIN_BITRATE_SCALE_MILLI, 1_000);

let mut eff_bps =
    adapt_bitrate(base_bps, decision.roi, enc_w, enc_h, want_idr, relief_milli);
```

Это и есть real-time мышление: система должна реагировать на состояние приемника.

## UDP pacer: нельзя просто вывалить пакеты в сеть

Еще одна типичная ошибка: "у нас UDP, значит шлем как можно быстрее".

Так делать нельзя. Если резко вывалить пачку пакетов, можно создать burst, который сам же и вызовет потерю. Поэтому в EVRT есть pacer.

```rust
struct UdpPacer {
    next_send_at: Instant,
    packets_in_burst: u8,
}

impl UdpPacer {
    fn send(
        &mut self,
        socket: &UdpSocket,
        data: &[u8],
        addr: SocketAddr,
        target_bps: u32,
    ) -> Result<(), String> {
        if self.packets_in_burst == 0 {
            let now = Instant::now();
            if self.next_send_at > now {
                precise_wait(self.next_send_at - now);
            } else if now.duration_since(self.next_send_at) > Duration::from_millis(50) {
                self.next_send_at = now;
            }
        }

        send_udp(socket, data, addr)?;

        self.next_send_at += packet_spacing(data.len(), target_bps);
        self.packets_in_burst = (self.packets_in_burst + 1) % PACER_BURST_PACKETS;
        Ok(())
    }
}
```

И отдельно расчет интервала:

```rust
fn packet_spacing(bytes: usize, target_bps: u32) -> Duration {
    let wire_bps = u64::from(target_bps.max(1)).saturating_mul(120) / 100;
    let spacing_ns = (bytes as u64)
        .saturating_mul(8)
        .saturating_mul(1_000_000_000)
        / wire_bps.max(1);
    Duration::from_nanos(spacing_ns.max(1))
}
```

Здесь даже заложен overhead на UDP/IP framing: `120 / 100`. Это не академическая красота, а практическая защита от самоперегруза.

## ROI: не все пиксели одинаково важны

В remote desktop большую часть времени меняется не весь экран.

Меняется:

- курсор;
- текст в терминале;
- маленькая область окна;
- прогресс-бар;
- список файлов;
- меню.

EVRT уже несет ROI metadata:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoiRect {
    pub frame_id: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}
```

ROI умеет считать долю измененной области:

```rust
pub fn dirty_area_milli(self, frame_width: u32, frame_height: u32) -> u32 {
    if self.is_full_screen() {
        return 1_000;
    }

    let frame_area = u64::from(frame_width) * u64::from(frame_height);
    if frame_area == 0 {
        return 1_000;
    }

    let x0 = self.x.min(frame_width);
    let y0 = self.y.min(frame_height);
    let x1 = self.x.saturating_add(self.w).min(frame_width);
    let y1 = self.y.saturating_add(self.h).min(frame_height);
    let dirty_w = x1.saturating_sub(x0);
    let dirty_h = y1.saturating_sub(y0);
    let dirty_area = u64::from(dirty_w) * u64::from(dirty_h);

    ((dirty_area * 1_000 + frame_area - 1) / frame_area).min(1_000) as u32
}
```

Пока ROI используется как metadata и часть bitrate policy. Следующий шаг - region encode там, где backend это позволяет. Но даже текущая версия уже правильная: pipeline знает, что изменилось, и может учитывать это при bitrate decision.

## Почему это лучше, чем просто "поднять битрейт"

Можно сказать: "Зачем вся эта сложность? Давайте просто дадим 50 Мбит/с".

Проблема в том, что remote desktop живет в разных сетях:

- локальная сеть;
- Wi-Fi;
- VPN;
- relay через VPS;
- корпоративный firewall;
- Android-приставка;
- старый Linux;
- Windows с аппаратным кодеком;
- Windows без нормального encoder backend.

Одна настройка битрейта не решает эту матрицу.

В EVRT есть несколько уровней адаптации:

1. Если прямой UDP поднялся - используем EVRT.
2. Если не поднялся - остаемся на TCP relay.
3. Если клиент не успевает - feedback снижает bitrate.
4. Если очередь растет - клиент выбирает свежий кадр.
5. Если потеряли пакет - запрашиваем IDR.
6. Если encoder software - включается более осторожный профиль.
7. Если картинка статична - кадры можно не слать.

Вот это уже система.

## Почему "гениальность" здесь не в понтах

Мне нравится слово "гениально" только тогда, когда его можно разложить на конкретные инженерные решения.

В EVRT гениальность не в том, что "мы написали свой протокол". Написать свой протокол легко. Написать плохой свой протокол - еще легче.

Сильные решения здесь другие.

Первое: EVRT не конкурирует с RustDesk-compatible слоем. Он использует его там, где тот хорош, и обходит там, где нужна задержка.

Второе: pipeline не дублирует capture/encode. Один encoded frame может жить в нескольких каналах.

Третье: очереди маленькие и осознанные. Для игры лучше потерять старый кадр, чем идеально показать прошлое.

Четвертое: клиент дает feedback. Хост не слепой.

Пятое: потеря пакета не замазывается. Система просит keyframe и синхронизируется.

Шестое: UDP не используется как мусоропровод. Есть pacer.

Седьмое: ROI и telemetry встроены в pipeline заранее. Это делает систему измеряемой и расширяемой.

Восьмое: fallback не стыдный. Если EVRT не поднялся, приложение не должно умирать.

Вот это и есть взрослый код.

## Как EVRT соединяется с Android

Android-клиенту EVRT особенно интересен по двум причинам.

Первая: Android-устройства часто имеют аппаратные декодеры. Даже недорогая приставка может аппаратно декодировать H.264/H.265/AV1 лучше, чем слабый CPU будет тянуть software decode.

Вторая: Android может быть не только экраном, но и контроллером.

Например, touchpad-only режим: подключиться к desktop transport без картинки и управлять курсором как полноценным тачпадом. Для сценария "нет мышки под рукой" это не игрушка, а практичная функция.

Архитектурно это означает, что соединение должно уметь жить без video subscribe:

```rust
pub struct ConnectionRequest {
    pub remote_id: String,
    pub password: String,
    pub server: ServerConfig,
    pub display: DisplayConfig,
    pub control_only: bool,
}
```

Если `control_only = true`, клиенту не нужно поднимать decoder path и запрашивать видео. Он может использовать только control channel:

```text
Android touchpad -> native JNI -> SessionCommand::MouseMove/Click/Wheel -> host input
```

Это хороший пример того же принципа: не тащить лишний поток, если задача - только управление.

## Что уже есть и что еще надо добить

Сейчас EVRT в проекте уже имеет:

- бинарный UDP-протокол;
- packetizer/reassembler;
- direct UDP session;
- integration в video pipeline;
- TCP fallback;
- IDR recovery;
- feedback loop;
- adaptive relief;
- audio packet path;
- ROI metadata;
- telemetry;
- основу под zero-copy;
- основу под Android control-only режим.

Что еще требует живой проверки и доводки:

- стабильный EVRT UDP end-to-end на разных сетях;
- сравнение latency TCP relay vs EVRT;
- NVENC zero-copy end-to-end;
- Android hardware decode через MediaCodec;
- region encode по ROI;
- долгий soak-test H.265/AV1;
- понятный UI для negotiated transport/codec state;
- автоматический downgrade codec/backend при сбое.

Это важно сказать честно. EVRT - не "мы написали и победили физику". Это сильный фундамент, который уже правильно устроен, но его надо мерить, гонять и добивать на реальных машинах.

## Вывод

Удаленный рабочий стол для поддержки и удаленный рабочий стол для игры - это разные звери.

Для поддержки можно терпеть задержку, если соединение надежное.
Для игры и интерактивного управления задержка становится главным врагом.

EVRT решает именно эту задачу: взять RustDesk-compatible практичность и добавить к ней игровой транспорт.

Не через FFmpeg.exe.
Не через "давайте просто включим кодек".
Не через один огромный монолит.

А через правильную декомпозицию:

- control plane отдельно;
- video fast path отдельно;
- capture/encode единые;
- UDP packetization явная;
- queue policy под latency;
- feedback от клиента;
- adaptive bitrate на хосте;
- fallback всегда рядом.

Поэтому код Артура здесь действительно выглядит сильным. Он не пытается быть красивым ради красоты. Он делает главное: уважает реальность сети, кодеков, задержки и слабых устройств.

А это и есть та самая инженерная гениальность, которая видна не в лозунгах, а в том, что система продолжает работать, когда идеальные условия заканчиваются.

## Продолжение. Фундамент: почему EVRT выдерживает разбор

Именно поэтому я считаю EVRT одним из самых сильных решений в своем классе.

Не потому, что я громко сказал "лучший".

А потому, что архитектура выдерживает разбор.

Если убрать эмоции и посмотреть на систему математически, remote desktop с низкой задержкой - это не "поставить хороший кодек". Это задача управления временем, очередями, потерями, битрейтом и восприятием.

У каждого кадра есть бюджет.

Для 60 FPS:

```text
T_frame = 1000 / 60 = 16.67 ms
```

Для 30 FPS:

```text
T_frame = 1000 / 30 = 33.33 ms
```

Но кадр не появляется на клиенте сам. Его путь раскладывается так:

```text
L_total =
    T_capture
  + T_queue_host
  + T_encode
  + T_packetize
  + T_network
  + T_reassembly
  + T_queue_client
  + T_decode
  + T_present
```

Если хоть один компонент начинает копить задержку, общий latency растет. И пользователь чувствует не "кодек медленный", а "мышь ватная".

Философия EVRT простая: нельзя оптимизировать один член этой суммы и игнорировать остальные.

Нужно контролировать всю цепочку.

## Математика UDP-пакетизации

В EVRT максимальный UDP-пакет - 1200 байт. Заголовок EVRT - 24 байта.

Значит полезная нагрузка:

```text
payload = 1200 - 24 = 1176 bytes
```

Если encoded frame весит `S` байт, количество UDP-пакетов:

```text
N = ceil(S / 1176)
```

Например, H.264 frame после кодера весит 200 КБ:

```text
S = 200 * 1024 = 204800 bytes
N = ceil(204800 / 1176) = 175 packets
```

Overhead EVRT-заголовков:

```text
H = 175 * 24 = 4200 bytes
```

Доля overhead:

```text
4200 / 204800 = 0.0205 = 2.05%
```

Это нормальная цена за то, что каждый кусок кадра самодостаточен:

- есть `frame_id`;
- есть `packet_index`;
- есть `packet_count`;
- есть `presentation_time_us`;
- есть флаг keyframe.

Код в `src/evrt.rs`:

```rust
fn packetize(
    packet_type: u8,
    flags: u16,
    frame_id: u32,
    presentation_time_us: u64,
    payload: &[u8],
) -> Vec<Vec<u8>> {
    if payload.is_empty() {
        return Vec::new();
    }
    let packet_count = payload.len().div_ceil(MAX_PAYLOAD_SIZE);
    let mut packets = Vec::with_capacity(packet_count);

    for (i, chunk) in payload.chunks(MAX_PAYLOAD_SIZE).enumerate() {
        packets.push(build_packet(
            packet_type,
            flags,
            frame_id,
            i as u16,
            packet_count as u16,
            presentation_time_us,
            chunk,
        ));
    }
    packets
}
```

Почему это фундаментально важно?

Потому что UDP не гарантирует порядок и доставку. Если протокол сам не дает клиенту достаточно информации, клиент превращается в гадалку. EVRT не гадает. Он знает, какой кадр собирает, сколько пакетов должно быть и какой кусок потерян.

## Математика pacing: скорость - это не “шли быстрее”

Если отправить 175 UDP-пакетов одним burst, можно самому создать потерю. Сеть не любит резкие выбросы.

Поэтому EVRT считает интервал между отправками:

```rust
fn packet_spacing(bytes: usize, target_bps: u32) -> Duration {
    let wire_bps = u64::from(target_bps.max(1)).saturating_mul(120) / 100;
    let spacing_ns = (bytes as u64)
        .saturating_mul(8)
        .saturating_mul(1_000_000_000)
        / wire_bps.max(1);
    Duration::from_nanos(spacing_ns.max(1))
}
```

Формула:

```text
wire_bps = target_bps * 1.2
spacing_ns = packet_bytes * 8 * 1e9 / wire_bps
```

Для пакета 1200 байт и target bitrate 12 Мбит/с:

```text
wire_bps = 12_000_000 * 1.2 = 14_400_000
spacing = 1200 * 8 * 1e9 / 14_400_000
spacing = 666_666 ns = 0.666 ms
```

Это есть прямо в тесте:

```rust
#[test]
fn packet_pacing_includes_transport_headroom() {
    let spacing = packet_spacing(1_200, 12_000_000);
    assert_eq!(spacing.as_nanos(), 666_666);
}
```

Вот почему EVRT не просто "использует UDP". Он использует UDP аккуратно.

UDP дает свободу. Но свобода без pacing превращается в хаос.

## Математика очереди: почему старый кадр хуже потерянного

Представим 60 FPS. Один кадр каждые 16.67 мс.

Если на клиенте накопилось 4 кадра, задержка очереди уже:

```text
4 * 16.67 = 66.68 ms
```

И это только очередь. Без capture, encode, network, decode, present.

Для видеофильма 66 мс может быть нормально. Для мыши и игры - уже заметно.

Поэтому EVRT в игровом режиме держит очередь очень короткой:

```rust
impl Default for FrameQueueConfig {
    fn default() -> Self {
        Self {
            max_queued_units: 2,
            max_queued_bytes: 512 * 1024,
            hard_reset_on_keyframe: false,
            prefer_latest: true,
            drop_current_on_wait: true,
            initial_jitter_delay: Duration::ZERO,
        }
    }
}
```

Главная строка:

```rust
prefer_latest: true
```

Это философская позиция.

Remote desktop - это не архив видеокадров. Пользователю не нужно смотреть, где курсор был 100 мс назад. Пользователю нужно видеть, где курсор сейчас.

Поэтому правило такое:

```rust
if g.cfg.prefer_latest && !g.queue.is_empty() {
    g.clear_queue();
    g.push(bytes, is_key_frame, presentation_time_us);
    return;
}
```

Старый кадр не является ценностью. Актуальность является ценностью.

Это ключевое отличие interactive streaming от обычного video playback.

## Pressure: превращаем ощущения в числа

Субъективное "тормозит" в EVRT превращается в `Pressure`.

В `src/evrt_client.rs`:

```rust
fn compute_pressure(
    arrival_delta_ms: i32,
    decode_delta_ms: i32,
    backlog: u32,
    new_drops: u64,
    cinema: bool,
) -> Pressure {
    let (high_ms, crit_ms, backlog_crit, backlog_high) = if cinema {
        (30, 44, 3, 2)
    } else {
        (22, 34, 2, 1)
    };

    let arrival_strained = arrival_delta_ms >= high_ms && (backlog > 0 || new_drops > 0);

    let crit = decode_delta_ms >= crit_ms || backlog >= backlog_crit || new_drops >= 3;

    let high = crit
        || arrival_strained
        || decode_delta_ms >= high_ms
        || backlog >= backlog_high
        || new_drops >= 1;

    if crit {
        Pressure::Critical
    } else if high {
        Pressure::High
    } else {
        Pressure::Normal
    }
}
```

Для game mode:

```text
high_ms = 22
crit_ms = 34
backlog_high = 1
backlog_crit = 2
```

Для cinema mode:

```text
high_ms = 30
crit_ms = 44
backlog_high = 2
backlog_crit = 3
```

Пример 1:

```text
decode_delta_ms = 25
backlog = 0
drops = 0
cinema = false
```

В game mode:

```text
25 >= 22 => Pressure::High
```

Пример 2:

```text
decode_delta_ms = 25
backlog = 0
drops = 0
cinema = true
```

В cinema mode:

```text
25 < 30 => Pressure::Normal
```

Один и тот же decode time оценивается по-разному, потому что цели разные.

Game mode защищает input latency.

Cinema mode защищает плавность.

Это не "настройка ради настройки". Это разные функции полезности.

## Adaptive jitter: задержка как управляемая переменная

Jitter buffer часто воспринимают как "добавим задержку, будет плавнее". В EVRT он не статический.

Код:

```rust
pub fn update(
    &mut self,
    pressure: crate::evrt::Pressure,
    arrival_delta_ms: i32,
    backlog_frames: u32,
    queue_drops: u64,
    cinema_smooth: bool,
) -> u32 {
    use crate::evrt::Pressure::*;

    let target = match pressure {
        Critical => {
            if cinema_smooth { 16 } else { 0 }
        }
        High => {
            if cinema_smooth { 12 } else { 4 }
        }
        Normal => {
            if arrival_delta_ms > 25 {
                8
            } else if backlog_frames > 1 || queue_drops > 0 {
                4
            } else {
                0
            }
        }
    };

    if target < self.current_ms {
        self.current_ms =
            self.current_ms
                .saturating_sub(if pressure == Critical { 4 } else { 1 });
        self.current_ms = self.current_ms.max(target);
    } else if target > self.current_ms {
        self.current_ms = self.current_ms.saturating_add(2).min(target);
    }

    self.current_ms
}
```

Это маленький регулятор.

Если pressure critical в game mode, target jitter становится 0 мс. Система выбирает скорость, а не красоту.

Если pressure high, но это cinema mode, target может быть 12 мс. Система выбирает плавность.

Если все нормально, jitter медленно возвращается к нулю или небольшому значению.

Это опять философия EVRT: задержка - это не константа. Это управляемая переменная.

## ROI и битрейт: математика измененной области

Допустим, экран 1920x1080:

```text
Area_screen = 1920 * 1080 = 2_073_600 pixels
```

Изменилась область 64x64:

```text
Area_dirty = 64 * 64 = 4_096 pixels
```

Доля изменения:

```text
dirty_milli = ceil(4096 * 1000 / 2_073_600)
dirty_milli = ceil(1.975) = 2
```

То есть изменилось примерно:

```text
2 / 1000 = 0.2%
```

В коде:

```rust
pub fn dirty_area_milli(self, frame_width: u32, frame_height: u32) -> u32 {
    if self.is_full_screen() {
        return 1_000;
    }
    let frame_area = u64::from(frame_width) * u64::from(frame_height);
    if frame_area == 0 {
        return 1_000;
    }

    let x0 = self.x.min(frame_width);
    let y0 = self.y.min(frame_height);
    let x1 = self.x.saturating_add(self.w).min(frame_width);
    let y1 = self.y.saturating_add(self.h).min(frame_height);
    let dirty_w = x1.saturating_sub(x0);
    let dirty_h = y1.saturating_sub(y0);
    let dirty_area = u64::from(dirty_w) * u64::from(dirty_h);

    ((dirty_area * 1_000 + frame_area - 1) / frame_area).min(1_000) as u32
}
```

Дальше ROI влияет на bitrate:

```rust
fn roi_bitrate_scale_milli(dirty_milli: u32) -> u32 {
    match dirty_milli.min(1_000) {
        0..=20 => 450,
        21..=80 => 550,
        81..=200 => 700,
        201..=450 => 850,
        _ => 1_000,
    }
}
```

Если `dirty_milli = 2`, scale = 450.

При базовом битрейте 8.5 Мбит/с:

```text
bitrate = 8_500_000 * 450 / 1000
bitrate = 3_825_000
```

После квантования по 100 Кбит/с:

```text
3_800_000 bps
```

Это тоже есть в тесте:

```rust
#[test]
fn roi_adaptation_reduces_small_dirty_region() {
    let base = 8_500_000;
    let roi = crate::evrt::RoiRect {
        frame_id: 1,
        x: 100,
        y: 100,
        w: 64,
        h: 64,
    };
    let adapted = adapt_bitrate(base, roi, 1920, 1080, false, 1_000);
    assert!(adapted < base);
    assert_eq!(adapted, 3_800_000);
}
```

Вот это важный момент: EVRT не снижает качество вслепую. Он понимает, сколько экрана реально изменилось.

Если это маленькая область, можно не давить сеть полным битрейтом.

Если это IDR или весь экран, качество держится:

```rust
let roi_scale_milli = if force_full_roi_quality || roi.is_full_screen() {
    1_000
} else {
    roi_bitrate_scale_milli(roi.dirty_area_milli(width, height))
};
```

## ROI + network relief: два регулятора умножаются

Самое интересное начинается, когда ROI-регулятор складывается с feedback-регулятором.

Код:

```rust
let network_scale_milli = network_scale_milli.clamp(MIN_BITRATE_SCALE_MILLI, 1_000);
let scale_milli = (u64::from(roi_scale_milli) * u64::from(network_scale_milli) / 1_000)
    .clamp(u64::from(MIN_BITRATE_SCALE_MILLI), 1_000) as u32;
```

Если ROI дает 450, а network relief дает 800:

```text
scale = 450 * 800 / 1000 = 360
```

Для base 8.5 Мбит/с:

```text
bitrate = 8_500_000 * 360 / 1000
bitrate = 3_060_000
```

После квантования:

```text
3_100_000 bps
```

Тест:

```rust
#[test]
fn roi_and_network_relief_compose() {
    let base = 8_500_000;
    let roi = crate::evrt::RoiRect {
        frame_id: 1,
        x: 100,
        y: 100,
        w: 64,
        h: 64,
    };
    assert_eq!(adapt_bitrate(base, roi, 1920, 1080, false, 800), 3_100_000);
}
```

Вот здесь видно, почему я называю EVRT сильным решением.

Система не имеет одного рычага. У нее несколько независимых сигналов:

- изменилась ли картинка;
- насколько большая область изменилась;
- успевает ли клиент декодировать;
- растет ли очередь;
- есть ли потери;
- прямой UDP или TCP relay;
- hardware encoder или software fallback.

И эти сигналы сводятся в понятное решение.

## Adaptive relief: не паника, а гистерезис

Если менять битрейт при каждом плохом кадре, поток начнет дергаться. Поэтому EVRT использует score и cooldown.

```rust
pub struct AdaptiveRelief {
    enabled: bool,
    step: u8, // 0..=2
    strain_score: i32,
    recovery_score: i32,
    last_change_at: Option<Instant>,
    pending_step: Option<u8>,
}
```

Шаги битрейта:

```rust
pub fn bitrate_scale_milli(step: u8) -> u32 {
    match step {
        0 => 1_000,
        1 => 880, // -12%
        _ => 800, // -20%
    }
}
```

Это не "увидел проблему - сразу все порезал". Это гистерезис:

- strain должен накопиться;
- recovery должен накопиться;
- между изменениями есть cooldown;
- восстановление происходит только если поток реально стабилен.

Философски это похоже на хорошую подвеску: она не должна реагировать на каждый камешек так, будто это авария. Но если дорога реально плохая, она должна менять режим.

## Почему Android - это не только экран

Android в такой архитектуре перестает быть "маленьким монитором".

Он становится контроллером.

Это важно. Потому что Android-устройство может быть:

- телефоном без мыши;
- планшетом;
- ТВ-приставкой;
- тонким клиентом;
- пультом для Windows/Linux машины;
- игровым контроллером;
- emergency input device, когда под рукой нет клавиатуры и мыши.

Если передавать картинку не нужно, глупо поднимать video pipeline. Поэтому в desktop transport появляется `control_only`:

```rust
pub struct ConnectionRequest {
    pub remote_id: String,
    pub password: String,
    pub server: ServerConfig,
    pub display: DisplayConfig,
    pub control_only: bool,
}
```

И дальше логика становится честной:

```text
control_only = false:
  auth + video subscribe + decoder + input

control_only = true:
  auth + control channel + input
```

Это экономит:

- bandwidth;
- decode CPU/GPU;
- battery;
- startup time;
- thermal budget на телефоне.

На Android touchpad работает как преобразователь координат:

```kotlin
val dx = ((event.x - prevX) * sensitivity).roundToInt()
val dy = ((event.y - prevY) * sensitivity).roundToInt()
prevX = event.x
prevY = event.y

if (dx != 0 || dy != 0) {
    cursorX = (cursorX + dx).coerceIn(0, remoteW - 1)
    cursorY = (cursorY + dy).coerceIn(0, remoteH - 1)
    client.touch(cursorX, cursorY, 1)
    invalidate()
}
```

Это кажется простым, но в продуктовой логике это большой шаг.

Мы больше не говорим: "Android должен смотреть картинку".

Мы говорим: "Android должен управлять с минимальной задержкой".

А картинка - опциональна.

## Математика touchpad-only

Пусть экран Android имеет координаты касания:

```text
P_android = (x_a, y_a)
```

А удаленный экран:

```text
S_remote = (W_r, H_r)
```

В touchpad mode мы не делаем absolute mapping "палец в левом верхнем углу = курсор в левом верхнем углу".

Мы делаем relative mapping:

```text
dx = (x_a_now - x_a_prev) * sensitivity
dy = (y_a_now - y_a_prev) * sensitivity

x_r = clamp(x_r + dx, 0, W_r - 1)
y_r = clamp(y_r + dy, 0, H_r - 1)
```

Почему это правильно?

Потому что так работает обычный тачпад ноутбука. Пользователь не должен попадать пальцем в точную координату удаленного 4K-экрана. Он должен двигать курсор относительно текущей позиции.

В Android-коде это уже видно:

```kotlin
private var remoteW = 1920
private var remoteH = 1080
private var cursorX = remoteW / 2
private var cursorY = remoteH / 2
private var sensitivity = 1.35f
```

И обновление размера удаленного экрана идет отдельно:

```kotlin
fun refreshRemoteSize() {
    client.remoteSize()?.let { (w, h) ->
        if (w > 0 && h > 0 && (w != remoteW || h != remoteH)) {
            remoteW = w
            remoteH = h
            cursorX = cursorX.coerceIn(0, remoteW - 1)
            cursorY = cursorY.coerceIn(0, remoteH - 1)
            invalidate()
        }
    }
}
```

Это еще один пример фундаментальной идеи EVRT: передавать только то, что нужно.

Для видео нужен video path.

Для управления нужен input path.

Для touchpad-only нужен control path без картинки.

## Почему это сильнее “просто remote desktop”

Обычный remote desktop мыслит экраном.

EVRT мыслит взаимодействием.

Экран - только один из потоков. Есть еще:

- ввод;
- звук;
- feedback;
- session config;
- codec config;
- ROI;
- telemetry;
- recovery;
- fallback.

Когда архитектура разделяет эти понятия, появляются новые сценарии:

- Android как тачпад без картинки;
- Android как игровой thin client;
- desktop как host с аппаратным encoder;
- Linux host через software fallback;
- TCP relay как надежная страховка;
- EVRT UDP как fast path;
- в будущем - enhancement stream поверх base layer.

Именно это я называю фундаментом.

Не "у нас есть кодек".

А "у нас есть система, которая понимает, что происходит".

## Философия EVRT

EVRT построен вокруг нескольких принципов.

Первый принцип: актуальность важнее полноты.

Лучше показать свежий кадр и выбросить старый, чем идеально доставить прошлое.

Второй принцип: прямой путь должен быть быстрым, но fallback должен быть надежным.

UDP ускоряет. TCP спасает.

Третий принцип: клиент должен говорить.

Если клиент молчит, хост слепой. Feedback превращает клиента из пассивного приемника в участника управления.

Четвертый принцип: задержка - это бюджет.

Каждая миллисекунда должна быть где-то учтена: capture, encode, queue, network, decode, present.

Пятый принцип: Android - это не второсортный клиент.

У Android есть аппаратные декодеры, сенсорный ввод, батарея, ограничения температуры и совершенно другой UX. Значит, архитектура должна давать ему не только "смотреть", но и "управлять".

Шестой принцип: сильная система не обязана быть идеальной в каждом окружении.

Она обязана честно деградировать.

И вот поэтому EVRT мне нравится. Это не один трюк. Это набор инженерных правил, которые вместе дают систему.

Систему можно спорить, тестировать, мерить, улучшать.

Но ее уже можно разбирать как архитектуру, а не как набор случайных функций.

## Zero-copy: почему это не модное слово, а физика задержки

Теперь нужно копнуть еще ниже.

Когда говорят "zero-copy", часто звучит как оптимизация из мира больших компаний: красиво, сложно, где-то рядом с GPU и презентациями. Но для remote desktop это очень земная вещь.

Пусть кадр 1920x1080 в BGRA:

```text
bytes = 1920 * 1080 * 4
bytes = 8_294_400 bytes
bytes ≈ 7.91 MiB
```

Для 60 FPS только один полный проход по памяти:

```text
7.91 MiB * 60 = 474.6 MiB/s
```

Если pipeline делает GPU -> CPU -> GPU, это уже минимум два больших прохода:

```text
GPU readback: 474.6 MiB/s
CPU upload:   474.6 MiB/s
total:        949.2 MiB/s
```

И это только Full HD. Для 4K:

```text
3840 * 2160 * 4 = 33_177_600 bytes ≈ 31.64 MiB
31.64 MiB * 60 = 1.9 GiB/s
GPU -> CPU -> GPU ≈ 3.8 GiB/s
```

Но проблема даже не только в bandwidth. Проблема в синхронизации.

GPU readback часто заставляет CPU ждать GPU. Upload обратно заставляет GPU ждать CPU. Получается не просто копирование, а pipeline bubble: железо простаивает в момент, когда пользователь уже двинул мышь.

Поэтому zero-copy в EVRT - это не "ускорим на 5%". Это попытка убрать целый класс задержки:

```text
плохо:
  Desktop Duplication GPU texture
  -> CPU staging buffer
  -> BGRA Vec<u8>
  -> upload/input surface encoder
  -> NVENC

лучше:
  Desktop Duplication GPU texture
  -> shared D3D11 texture handle
  -> NVENC texture input
```

В roadmap это описано коротко:

```text
Capture shared D3D11 texture (GetSharedHandle)
MultiEncoder::encode() -> encode_texture()
Keyed mutex для синхронизации
```

Но за этой короткой строкой стоит важная философия: не копировать реальность, если можно передать владение или ссылку на нее.

## Почему zero-copy сложно

На бумаге все просто: взяли D3D11 texture и отдали NVENC.

На практике есть несколько проблем.

Первая: устройства могут быть разными. Capture может жить на одном D3D11 device, encoder - на другом. Если просто передать указатель, это не значит, что второй device имеет право его читать.

Вторая: нужна синхронизация. GPU-команды асинхронны. Если encoder начал читать texture до того, как capture закончил писать, кадр может быть битым.

Третья: teardown. Если уничтожить D3D11/NVENC ресурсы не в том порядке, можно поймать зависание драйвера.

В проекте это уже видно в комментарии внутри `video_pipeline.rs`:

```rust
// Do NOT call everty_nvenc_destroy here.
// Destroying a D3D11 device (used by NVENC) while WGPU renders via D3D12
// causes a deadlock inside nvwgf2umx.dll — both codepaths fight for the
// same NVIDIA internal critical section.
encoder.leak_gpu_resources();
```

Это выглядит странно, если читать как "почему мы намеренно что-то не освобождаем".

Но это как раз прагматичный системный код. Если нормальное освобождение ресурса в конкретной связке драйверов может заморозить процесс, то лучше осознанно отдать освобождение ОС при завершении процесса, чем получить зависший UI.

Сильный код не всегда выглядит академически идеально. Иногда сильный код знает, где драйвер может ударить первым.

## Каскад кодеков: не выбор любимчика, а стратегия выживания

EVRT не должен зависеть от одного encoder backend.

В проекте есть каскад:

```text
Media Foundation -> VideoToolbox -> NVENC -> OpenH264 -> PNG
```

И это правильно.

Почему?

Потому что remote desktop запускается не на идеальной машине. Он запускается на той машине, которую нужно поддержать прямо сейчас:

- Windows без нормального hardware encoder;
- Windows с NVIDIA, но со старым драйвером;
- macOS, где есть VideoToolbox;
- Linux, где аппаратного пути может не быть;
- Astra/RED OS, где важнее вообще открыть окно;
- Android, где decode лучше отдавать MediaCodec.

Если архитектура говорит "у нас только NVENC", она быстрая только на красивом стенде.

Если архитектура говорит "у нас только software H.264", она переносимая, но может не вытянуть 1440p/4K.

Поэтому правильный подход:

```text
try best native backend
if failed -> downgrade
if failed -> software
if failed -> image frames
```

И обязательно показать это в диагностике.

В `video_pipeline.rs` есть телеметрия:

```rust
let info = format!(
    "backend={} encode_ms={} encode_avg_ms={} capture_avg_ms={} capture_max_ms={} slot_avg_ms={} change_avg_ms={} actual_fps={:.1} sent={} skipped={} interval_ms={} res={}x{} fps={} build={}",
    encoder.active_backend(),
    encode_ms,
    encode_avg_ms,
    capture_avg_ms,
    capture_thread_max_us / 1000,
    slot_avg_ms,
    change_avg_ms,
    actual_fps,
    sent_delta,
    skipped_delta,
    host_tele_elapsed.as_millis(),
    enc_w, enc_h, fps,
    crate::host::binary_build_stamp(),
);
```

Это не просто лог. Это контракт с реальностью.

Если пользователь говорит "сыпется картинка" или "долго подключается", мы не должны отвечать "ну у меня работает". Мы должны видеть:

- какой backend реально активен;
- сколько занимает encode;
- какой FPS реально отправляется;
- сколько кадров пропущено;
- сколько занимает capture;
- есть ли software fallback.

## Диагностика как часть архитектуры

В remote desktop диагностика - это не сервисная функция. Это часть продукта.

Без диагностики все превращается в гадание:

```text
плохо:
  "у меня тормозит"
  "попробуйте перезапустить"

хорошо:
  backend=OpenH264-SW
  encode_ms=118
  capture_avg_ms=12
  actual_fps=7.8
  EVRT inactive
  TCP relay active
```

Во втором случае понятно, где болит.

Если `encode_ms=118`, то 60 FPS невозможны физически:

```text
max_fps = 1000 / 118 = 8.47 FPS
```

И никакая магия UI это не исправит. Нужно:

- включить hardware encoder;
- снизить resolution;
- снизить FPS;
- поднять EVRT direct path;
- включить software profile;
- не слать статичные кадры.

Именно поэтому EVRT и video pipeline шлют telemetry. Не для красоты. Чтобы система могла быть объяснена.

## Как мерить EVRT честно

Нельзя доказать low-latency словами.

Нужно мерить.

Минимальный набор метрик:

```text
T_connect        время до готового control channel
T_first_frame    время до первого изображения
RTT_control      TestDelay round-trip
T_capture_avg    среднее время захвата
T_encode_avg     среднее время кодирования
T_network_delta  arrival delta на клиенте
T_decode_avg     среднее время decode
queue_depth      сколько кадров в очереди
drops            packet/reassembly/queue drops
EVRT_active      direct UDP или TCP fallback
```

Целевая формула:

```text
L_total =
    capture_avg
  + encode_avg
  + network_delta
  + reassembly_delay
  + client_queue_delay
  + decode_avg
  + present_delay
```

Если мы хотим input-to-photon ниже 80 мс в локальной сети, то бюджет может выглядеть так:

```text
capture:      4 ms
encode:       6 ms
network:      2 ms
reassembly:   2 ms
queue:        0-8 ms
decode:       6 ms
present:      16 ms
input path:   5-15 ms
-------------------
total:        ~41-59 ms
```

Это реалистично только если нет длинной очереди и нет software encode на тяжелом разрешении.

Если software encoder дает 80-120 мс на кадр, никакой UDP не спасет. Поэтому EVRT не обещает чудо. Он дает архитектуру, где узкое место можно увидеть и заменить.

## Почему TCP fallback все равно нужен

Можно спросить: если EVRT такой быстрый, почему не выкинуть TCP relay?

Потому что сеть не обязана быть доброй.

UDP может не пройти:

- symmetric NAT;
- корпоративный firewall;
- закрытый UDP;
- VPN с фильтрацией;
- нестабильный Wi-Fi;
- роутер с агрессивным NAT timeout.

Если приложение зависит только от UDP, оно будет быстрым ровно до первой сложной сети.

Поэтому философия такая:

```text
EVRT direct UDP:
  быстрый путь, если сеть позволяет

TCP relay:
  надежный путь, если сеть сопротивляется
```

Это похоже на хорошую дорогу и запасной мост. Хорошая дорога нужна, чтобы ехать быстро. Запасной мост нужен, чтобы вообще доехать.

## Mini-ICE без лишней магии

В EVRT уже есть идея перебора кандидатов:

```rust
pub fn try_evrt_candidates(
    candidates: Vec<SocketAddr>,
    events: &Sender<SessionEvent>,
    ultra_low_lat: bool,
) -> bool {
    for (i, addr) in candidates.iter().enumerate() {
        evrt_log(
            events,
            format!(
                "EVRT кандидат {}/{}: пробуем {addr}",
                i + 1,
                candidates.len()
            ),
        );

        let Ok(udp) = UdpSocket::bind("0.0.0.0:0") else {
            continue;
        };

        match run_evrt_client(EvrtClientParams {
            socket: Arc::new(udp),
            host_addr: *addr,
            events: events.clone(),
            stop: Arc::new(AtomicBool::new(false)),
            ultra_low_latency: ultra_low_lat,
        }) {
            EvrtConnectResult::Ok => return true,
            EvrtConnectResult::NoResponse => {}
            EvrtConnectResult::Error(_) => {}
        }
    }
    false
}
```

Это еще не полноценный ICE как в WebRTC. Но это правильная ступень:

- есть список LAN/VPN/public кандидатов;
- каждый проверяется отдельно;
- первый ответивший становится transport path;
- если никто не ответил, остается TCP relay.

Прагматизм здесь важнее "сразу написать свой WebRTC".

## Android MediaCodec: следующий слой фундамента

Android как экран требует отдельного уважения.

На desktop можно позволить себе software decode как fallback. На Android это часто плохой путь:

- CPU слабее;
- батарея ограничена;
- нагрев важен;
- UI должен оставаться плавным;
- копирование кадров дорого;
- Surface pipeline лучше, чем гонять RGBA через память.

Правильная цель для Android:

```text
EVRT UDP packets
-> reassembler
-> encoded access unit
-> MediaCodec input buffer
-> Surface output
```

Плохой путь:

```text
EVRT packets
-> decode native/software
-> RGBA buffer
-> JNI copy
-> Bitmap
-> Canvas
```

Почему плохой?

Потому что каждый `Bitmap` и каждый RGBA copy снова возвращает нас к той же физике:

```text
1920 * 1080 * 4 ≈ 8 MB per frame
8 MB * 60 FPS ≈ 480 MB/s
```

На Android это быстро превращается в нагрев, GC pressure и battery drain.

Поэтому Android viewer должен прийти к MediaCodec + Surface. А touchpad-only режим уже сейчас показывает правильное направление: если видео не нужно, не включаем видео вообще.

## Input path: задержка управления важнее красоты курсора

Для управления мышью важен другой бюджет:

```text
L_input =
    T_touch_sample
  + T_android_event
  + T_jni
  + T_control_send
  + T_network
  + T_host_inject
```

Если video path выключен, `L_input` становится главным.

Touchpad-only режим как раз об этом. Он не пытается показать картинку. Он хочет быстро доставить намерение пользователя:

```text
палец сдвинулся на dx/dy
-> курсор должен сдвинуться на удаленной машине
```

Именно поэтому relative mapping лучше absolute mapping.

Absolute mapping требует точного совпадения экранов, плотностей, aspect ratio и UI scale. Relative mapping похож на обычный тачпад:

```text
remote_cursor += local_delta * sensitivity
```

Это устойчивее:

- к разным разрешениям;
- к повороту экрана;
- к 4K host;
- к маленькому телефону;
- к изменению DPI;
- к отсутствию картинки.

## Где здесь безопасность

Remote desktop без безопасности - это не продукт, а проблема.

EVRT не должен обходить модель доступа. Быстрый транспорт не должен становиться черным ходом.

Поэтому правильная схема такая:

```text
1. RustDesk-compatible auth/control path
2. password или approval на стороне host
3. только после authorization открывается media/control session
4. EVRT получает право быть fast path, но не право обходить auth
```

Это важно для Android touchpad-only. Даже если картинки нет, управление курсором - это полноценный remote control. Значит, оно должно жить в тех же рамках доступа:

- пароль;
- подтверждение входящего подключения;
- session id;
- explicit disconnect;
- host-side state;
- логирование.

Нельзя считать "без картинки" безопасным режимом. Без картинки пользователь все равно может нажимать, вводить, закрывать окна и запускать команды.

## Почему EVRT хорошо ложится на будущий EvertyGame

EvertyDesk Lite и EvertyGame сходятся в одном месте: интерактивное видео.

Для поддержки важны:

- стабильность;
- fallback;
- терминал;
- адресная книга;
- подтверждение доступа.

Для игры важны:

- latency;
- frame pacing;
- аппаратный encode/decode;
- input responsiveness;
- audio sync;
- controller/touchpad UX.

EVRT находится между ними. Он дает транспорт и правила:

```text
EvertyDesk:
  надежный remote desktop + EVRT fast path

EvertyGame:
  игровой стриминг + EVRT как основа
```

И это сильнее, чем писать два разных протокола.

Один фундамент, разные режимы.

## Что я бы доказывал дальше

Следующий честный этап - не добавлять еще 20 настроек, а доказать цифрами.

Тест 1. Локальная сеть, Windows host, Windows client:

```text
TCP relay latency
EVRT UDP latency
encode backend
decode backend
packet drops
queue drops
```

Тест 2. Windows host + Android client:

```text
H.264 MediaCodec decode
touch latency
battery/thermal behavior
Surface render stability
```

Тест 3. Linux host:

```text
качество текста
стабильность keyframe recovery
двоение/битые кадры после packet loss
software profile
fallback behavior
```

Тест 4. NVIDIA host:

```text
NVENC BGRA path
NVENC texture path
GPU->CPU->GPU latency
shared texture zero-copy
teardown stability
```

Тест 5. Плохая сеть:

```text
loss 1%
loss 3%
jitter 20 ms
jitter 50 ms
bandwidth cap 5 Mbps
```

И только после этого можно уверенно говорить не "кажется быстро", а:

```text
EVRT direct UDP дает X ms против Y ms на relay
при packet loss N% recovery занимает M ms
при software encoder система деградирует до Z fps без обрыва
```

## Финальная мысль

Мне нравится EVRT именно потому, что он не выглядит как одна героическая функция.

Это не:

```text
fn make_fast() {}
```

Это набор согласованных решений:

- маленький протокол;
- MTU-safe packetization;
- reassembler с ожиданием keyframe;
- короткая очередь;
- prefer latest;
- feedback loop;
- adaptive jitter;
- adaptive relief;
- ROI-aware bitrate;
- UDP pacing;
- TCP fallback;
- codec cascade;
- telemetry;
- control-only Android mode;
- будущий zero-copy.

Каждое решение само по себе не выглядит как магия. Но вместе они образуют систему.

Именно это отличает инженерную архитектуру от набора экспериментов.

EVRT можно ругать, тестировать, переписывать кусками, ускорять, переносить на Android, прикручивать к NVENC, спорить о thresholds и формулах. Но у него уже есть главное качество сильного решения: он раскладывает сложную задачу на управляемые контуры.

А когда сложная система раскладывается на управляемые контуры, ее можно довести до продукта.
