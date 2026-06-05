// =============================================================================
// EVRT Protocol — разработан Артуром Валиевым (Artur Valiev)
// Оригинальная реализация: EvertyGame (C#, https://github.com/djvaliev)
// Rust-порт для EvertyDesk Lite выполнен на основе оригинальных алгоритмов.
//
// Протокол, алгоритмы адаптивной буферизации, система давления (pressure),
// логика FeedbackLoop и LatestAccessUnitQueue — интеллектуальная собственность
// Артура Валиева, разработанная в течение нескольких лет работы над EvertyGame.
// =============================================================================

//! Умная очередь видеокадров — порт `LatestAccessUnitQueue` из EvertyGame.
//!
//! Ключевые свойства:
//! - `prefer_latest`: при переполнении дропает старые, берёт новейший кадр
//! - `hard_reset_on_keyframe`: IDR очищает весь буфер — мгновенный sync
//! - `jitter_buffer_delay`: динамически регулируемая задержка (0 = нулевая)
//! - `waiting_for_keyframe`: после потери пакетов ждёт IDR, не проигрывает битый поток
//!
//! # Схема работы
//!
//! ```text
//! UDP reassembler          FrameQueue            декодер
//! ─────────────────────────────────────────────────────
//! frame ready  ──enqueue──►  [0][1][2]  ──dequeue──► decode
//!                          ↑           ↑
//!                       drop old    wait jitter
//! ```

// Полная API-поверхность EVRT-протокола: часть методов — публичный
// интерфейс для будущего использования (enhancement layer, audio config, jitter API).
#![allow(dead_code)]

use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

// ─── публичные типы ───────────────────────────────────────────────────────────

/// Один кадр в очереди.
#[derive(Debug)]
struct QueuedUnit {
    bytes:               Vec<u8>,
    is_key_frame:        bool,
    presentation_time_us: u64,
    enqueued_at:         Instant,
}

/// Снимок статистики очереди.
#[derive(Debug, Clone, Default)]
pub struct QueueStats {
    pub queued_units:       usize,
    pub queued_bytes:       usize,
    pub dropped_units:      u64,
    pub waiting_for_keyframe: bool,
}

/// Конфигурация очереди.
#[derive(Debug, Clone)]
pub struct FrameQueueConfig {
    /// Максимальное количество кадров в очереди.
    pub max_queued_units: usize,
    /// Максимальный суммарный объём в байтах.
    pub max_queued_bytes: usize,
    /// IDR сбрасывает всё накопленное — нулевая задержка при переподключении.
    pub hard_reset_on_keyframe: bool,
    /// При переполнении дропать старые кадры и брать новейший.
    /// `true` = минимальная задержка (режим игры).
    /// `false` = дропать все и ждать keyframe (режим медиа).
    pub prefer_latest: bool,
    /// При переходе в `waiting_for_keyframe` сбрасывать текущий кадр (если идёт декод).
    pub drop_current_on_wait: bool,
    /// Начальная задержка jitter-буфера. 0 = нулевая.
    pub initial_jitter_delay: Duration,
}

impl Default for FrameQueueConfig {
    /// Настройки для режима игры (максимальная скорость, минимальная задержка).
    fn default() -> Self {
        Self {
            max_queued_units:       2,
            max_queued_bytes:       512 * 1024,
            hard_reset_on_keyframe: false,
            prefer_latest:          true,
            drop_current_on_wait:   true,
            initial_jitter_delay:   Duration::ZERO,
        }
    }
}

impl FrameQueueConfig {
    /// Настройки для режима медиа (плавность важнее задержки).
    pub fn cinema() -> Self {
        Self {
            max_queued_units:       4,
            max_queued_bytes:       2 * 1024 * 1024,
            hard_reset_on_keyframe: false,
            prefer_latest:          false,
            drop_current_on_wait:   false,
            initial_jitter_delay:   Duration::from_millis(16),
        }
    }
}

// ─── внутреннее состояние ────────────────────────────────────────────────────

struct Inner {
    queue:                VecDeque<QueuedUnit>,
    queued_bytes:         usize,
    current:              Option<QueuedUnit>,
    waiting_for_keyframe: bool,
    dropped_units:        u64,
    closed:               bool,
    jitter_delay:         Duration,
    cfg:                  FrameQueueConfig,
}

impl Inner {
    fn new(cfg: FrameQueueConfig) -> Self {
        let jitter = cfg.initial_jitter_delay;
        Self {
            queue:                VecDeque::new(),
            queued_bytes:         0,
            current:              None,
            waiting_for_keyframe: false,
            dropped_units:        0,
            closed:               false,
            jitter_delay:         jitter,
            cfg,
        }
    }

    fn stats(&self) -> QueueStats {
        let current_bytes = self.current.as_ref().map(|u| u.bytes.len()).unwrap_or(0);
        QueueStats {
            queued_units:         self.queue.len() + self.current.as_ref().map(|_| 1).unwrap_or(0),
            queued_bytes:         self.queued_bytes + current_bytes,
            dropped_units:        self.dropped_units,
            waiting_for_keyframe: self.waiting_for_keyframe,
        }
    }

    fn clear_queue(&mut self) {
        self.dropped_units += self.queue.len() as u64;
        self.queue.clear();
        self.queued_bytes = 0;
    }

    fn drop_current(&mut self) {
        if self.current.take().is_some() {
            self.dropped_units += 1;
        }
    }

    fn push(&mut self, bytes: Vec<u8>, is_key_frame: bool, presentation_time_us: u64) {
        let unit_bytes = bytes.len();
        self.queue.push_back(QueuedUnit {
            bytes,
            is_key_frame,
            presentation_time_us,
            enqueued_at: Instant::now(),
        });
        self.queued_bytes += unit_bytes;
    }
}

// ─── публичный интерфейс ──────────────────────────────────────────────────────

/// Умная очередь кадров с адаптивным jitter-буфером.
pub struct FrameQueue {
    state:  Arc<(Mutex<Inner>, Condvar)>,
}

impl FrameQueue {
    pub fn new(cfg: FrameQueueConfig) -> Self {
        Self {
            state: Arc::new((Mutex::new(Inner::new(cfg)), Condvar::new())),
        }
    }

    /// Клон хендла для использования из другого потока.
    pub fn handle(&self) -> FrameQueueHandle {
        FrameQueueHandle { state: self.state.clone() }
    }

    /// Поместить кадр в очередь.
    pub fn enqueue(&self, bytes: Vec<u8>, is_key_frame: bool, presentation_time_us: u64) {
        let (lock, cvar) = &*self.state;
        let mut g = lock.lock().unwrap();
        Self::enqueue_inner(&mut g, bytes, is_key_frame, presentation_time_us);
        cvar.notify_all();
    }

    fn enqueue_inner(
        g: &mut Inner,
        bytes: Vec<u8>,
        is_key_frame: bool,
        presentation_time_us: u64,
    ) {
        if g.closed {
            return;
        }

        // ── ждём keyframe после потери ────────────────────────────────────────
        if g.waiting_for_keyframe {
            if !is_key_frame {
                g.dropped_units += 1;
                return;
            }
            // IDR пришёл — сбрасываем и принимаем
            g.clear_queue();
            if g.cfg.drop_current_on_wait {
                g.drop_current();
            }
            g.waiting_for_keyframe = false;
            g.push(bytes, is_key_frame, presentation_time_us);
            return;
        }

        // ── hard reset на IDR ─────────────────────────────────────────────────
        if g.cfg.hard_reset_on_keyframe && is_key_frame
            && (g.current.is_some() || !g.queue.is_empty())
        {
            g.clear_queue();
            g.drop_current();
            g.waiting_for_keyframe = false;
            g.push(bytes, is_key_frame, presentation_time_us);
            return;
        }

        // ── prefer_latest: дропаем всё кроме нового ──────────────────────────
        if g.cfg.prefer_latest && !g.queue.is_empty() {
            g.clear_queue();
            g.push(bytes, is_key_frame, presentation_time_us);
            return;
        }

        // ── проверка переполнения ─────────────────────────────────────────────
        let overflow_units = g.queue.len() >= g.cfg.max_queued_units;
        let overflow_bytes = g.queued_bytes + bytes.len() > g.cfg.max_queued_bytes;

        if overflow_units || overflow_bytes {
            g.clear_queue();
            if g.cfg.drop_current_on_wait {
                g.drop_current();
            }
            if is_key_frame || g.current.is_none() {
                g.waiting_for_keyframe = false;
                g.push(bytes, is_key_frame, presentation_time_us);
            } else {
                g.dropped_units += 1;
                g.waiting_for_keyframe = true;
            }
            return;
        }

        // ── нормальный путь ───────────────────────────────────────────────────
        g.push(bytes, is_key_frame, presentation_time_us);
    }

    /// Извлечь следующий кадр. Блокирует до появления кадра или отмены.
    /// Возвращает `None` когда очередь закрыта.
    pub fn dequeue(&self, cancel: &std::sync::atomic::AtomicBool)
        -> Option<(Vec<u8>, bool, u64)>
    {
        use std::sync::atomic::Ordering;
        let (lock, cvar) = &*self.state;
        let mut g = lock.lock().unwrap();

        loop {
            if g.closed || cancel.load(Ordering::Relaxed) {
                return None;
            }

            // Переместить из queue → current если current пуст
            if g.current.is_none() {
                if let Some(unit) = g.queue.pop_front() {
                    g.queued_bytes = g.queued_bytes.saturating_sub(unit.bytes.len());
                    g.current = Some(unit);
                }
            }

            if let Some(ref unit) = g.current {
                // Jitter-буфер: если задержка задана и в очереди ничего нет — ждём
                if g.jitter_delay > Duration::ZERO && g.queue.is_empty() {
                    let age = unit.enqueued_at.elapsed();
                    if age < g.jitter_delay {
                        let wait = g.jitter_delay - age;
                        let timeout = wait.min(Duration::from_millis(20));
                        g = cvar.wait_timeout(g, timeout).unwrap().0;
                        continue;
                    }
                }

                let unit = g.current.take().unwrap();
                return Some((unit.bytes, unit.is_key_frame, unit.presentation_time_us));
            }

            // Ничего нет — ждём с коротким таймаутом
            g = cvar.wait_timeout(g, Duration::from_millis(10)).unwrap().0;
        }
    }

    /// Обновить задержку jitter-буфера на лету.
    pub fn set_jitter_delay(&self, delay: Duration) {
        let (lock, cvar) = &*self.state;
        let mut g = lock.lock().unwrap();
        g.jitter_delay = delay;
        cvar.notify_all();
    }

    /// Сбросить в режим ожидания keyframe (после потери пакетов).
    pub fn wait_for_keyframe(&self) {
        let (lock, cvar) = &*self.state;
        let mut g = lock.lock().unwrap();
        if g.closed { return; }
        g.clear_queue();
        if g.cfg.drop_current_on_wait {
            g.drop_current();
        }
        g.waiting_for_keyframe = true;
        cvar.notify_all();
    }

    /// Полный сброс.
    pub fn flush(&self, wait_for_keyframe: bool) {
        let (lock, cvar) = &*self.state;
        let mut g = lock.lock().unwrap();
        g.clear_queue();
        g.drop_current();
        g.waiting_for_keyframe = wait_for_keyframe;
        cvar.notify_all();
    }

    /// Текущая статистика.
    pub fn stats(&self) -> QueueStats {
        let (lock, _) = &*self.state;
        lock.lock().unwrap().stats()
    }

    /// Закрыть очередь — все ожидающие `dequeue` вернут `None`.
    pub fn close(&self) {
        let (lock, cvar) = &*self.state;
        let mut g = lock.lock().unwrap();
        g.closed = true;
        g.clear_queue();
        g.drop_current();
        cvar.notify_all();
    }
}

/// Клон хендла очереди — можно передавать в другой поток.
#[derive(Clone)]
pub struct FrameQueueHandle {
    state: Arc<(Mutex<Inner>, Condvar)>,
}

impl FrameQueueHandle {
    pub fn enqueue(&self, bytes: Vec<u8>, is_key_frame: bool, presentation_time_us: u64) {
        let (lock, cvar) = &*self.state;
        let mut g = lock.lock().unwrap();
        FrameQueue::enqueue_inner(&mut g, bytes, is_key_frame, presentation_time_us);
        cvar.notify_all();
    }

    pub fn set_jitter_delay(&self, delay: Duration) {
        let (lock, cvar) = &*self.state;
        let mut g = lock.lock().unwrap();
        g.jitter_delay = delay;
        cvar.notify_all();
    }

    pub fn wait_for_keyframe(&self) {
        let (lock, cvar) = &*self.state;
        let mut g = lock.lock().unwrap();
        if g.closed { return; }
        g.clear_queue();
        if g.cfg.drop_current_on_wait { g.drop_current(); }
        g.waiting_for_keyframe = true;
        cvar.notify_all();
    }

    pub fn stats(&self) -> QueueStats {
        let (lock, _) = &*self.state;
        lock.lock().unwrap().stats()
    }

    pub fn close(&self) {
        let (lock, cvar) = &*self.state;
        let mut g = lock.lock().unwrap();
        g.closed = true;
        g.clear_queue();
        g.drop_current();
        cvar.notify_all();
    }
}

// ─── Reassembler ─────────────────────────────────────────────────────────────
//
// Порт FrameReassembler + AccessUnitChannelReassembler из EvertyGame.
// Собирает UDP-пакеты в полные кадры.

use crate::evrt::EvrtPacket;

/// Один незавершённый кадр в процессе сборки.
struct FrameAssembly {
    frame_id:             u32,
    packet_count:         u16,
    is_key_frame:         bool,
    presentation_time_us: u64,
    parts:                Vec<Option<Vec<u8>>>,
    received:             u16,
    first_packet_at:      Instant,
}

impl FrameAssembly {
    fn new(frame_id: u32, packet_count: u16, is_key_frame: bool, pts: u64) -> Self {
        Self {
            frame_id,
            packet_count,
            is_key_frame,
            presentation_time_us: pts,
            parts: vec![None; packet_count as usize],
            received: 0,
            first_packet_at: Instant::now(),
        }
    }

    fn set(&mut self, index: u16, payload: Vec<u8>) -> bool {
        let idx = index as usize;
        if idx >= self.parts.len() || self.parts[idx].is_some() {
            return false;
        }
        self.parts[idx] = Some(payload);
        self.received += 1;
        true
    }

    fn is_complete(&self) -> bool {
        self.received == self.packet_count
    }

    fn join(self) -> Vec<u8> {
        let mut out = Vec::new();
        for part in self.parts {
            if let Some(p) = part {
                out.extend_from_slice(&p);
            }
        }
        out
    }

    /// Время сборки в мс.
    fn assembly_delay_ms(&self) -> i32 {
        self.first_packet_at.elapsed().as_millis().min(i32::MAX as u128) as i32
    }
}

/// Сборщик кадров из одного канала (base или enhancement).
pub struct ChannelReassembler {
    frames:                  std::collections::HashMap<u32, FrameAssembly>,
    latest_codec_config:     Option<Vec<u8>>,
    latest_frame_id_seen:    Option<u32>,
    latest_completed_id:     Option<u32>,
    waiting_after_loss:      bool,
    dropped_frames:          u64,
}

impl ChannelReassembler {
    pub fn new() -> Self {
        Self {
            frames:               std::collections::HashMap::new(),
            latest_codec_config:  None,
            latest_frame_id_seen: None,
            latest_completed_id:  None,
            waiting_after_loss:   false,
            dropped_frames:       0,
        }
    }

    pub fn reset(&mut self) {
        self.frames.clear();
        self.latest_frame_id_seen = None;
        self.latest_completed_id  = None;
        self.waiting_after_loss   = false;
    }

    pub fn set_codec_config(&mut self, payload: Vec<u8>) {
        self.latest_codec_config = Some(payload);
    }

    /// Принять один UDP-пакет.
    /// Возвращает `Some((bytes, is_key, assembly_ms, pts))` когда кадр собран.
    pub fn on_packet(
        &mut self,
        pkt: &EvrtPacket,
    ) -> Option<(Vec<u8>, bool, i32, u64)> {
        // Базовые проверки
        if pkt.packet_count == 0 || pkt.packet_index >= pkt.packet_count {
            self.dropped_frames += 1;
            return None;
        }

        // Уже обработанный или слишком старый
        if let Some(completed) = self.latest_completed_id {
            if pkt.frame_id <= completed {
                return None;
            }
        }
        if let Some(seen) = self.latest_frame_id_seen {
            if pkt.frame_id < seen {
                return None;
            }
        }

        // Ждём IDR после потери
        if self.waiting_after_loss && !pkt.is_key_frame() {
            self.dropped_frames += 1;
            return None;
        }

        // Новый frame_id — дропаем незавершённые старые
        if self.latest_frame_id_seen.map(|s| pkt.frame_id > s).unwrap_or(true) {
            let had_incomplete = self.drop_older_than(pkt.frame_id);
            self.latest_frame_id_seen = Some(pkt.frame_id);

            if had_incomplete && !pkt.is_key_frame() {
                self.waiting_after_loss = true;
                self.dropped_frames += 1;
                return None;
            }
        }

        if pkt.is_key_frame() {
            self.waiting_after_loss = false;
            self.drop_older_than(pkt.frame_id);
        }

        // Собираем кадр
        let assembly = self.frames
            .entry(pkt.frame_id)
            .or_insert_with(|| FrameAssembly::new(
                pkt.frame_id,
                pkt.packet_count,
                pkt.is_key_frame(),
                pkt.presentation_time_us,
            ));

        if assembly.packet_count != pkt.packet_count {
            return None; // несовместимые пакеты
        }
        if !assembly.set(pkt.packet_index, pkt.payload.clone()) {
            return None; // дубль
        }
        if !assembly.is_complete() {
            return None;
        }

        // Кадр собран
        let assembly = self.frames.remove(&pkt.frame_id).unwrap();
        self.latest_completed_id = Some(assembly.frame_id);
        let delay_ms = assembly.assembly_delay_ms();
        let pts      = assembly.presentation_time_us;
        let key      = assembly.is_key_frame;
        let bytes    = assembly.join();

        if key {
            self.waiting_after_loss = false;
            // Prepend codec config (SPS/PPS) к keyframe
            if let Some(ref cfg) = self.latest_codec_config {
                let mut combined = cfg.clone();
                combined.extend_from_slice(&bytes);
                return Some((combined, true, delay_ms, pts));
            } else {
                // Нет конфига — ждём следующего
                self.waiting_after_loss = true;
                self.dropped_frames += 1;
                return None;
            }
        }

        if self.waiting_after_loss {
            self.dropped_frames += 1;
            return None;
        }

        Some((bytes, false, delay_ms, pts))
    }

    pub fn dropped_frames(&self) -> u64 { self.dropped_frames }

    fn drop_older_than(&mut self, frame_id: u32) -> bool {
        let had = !self.frames.is_empty();
        self.frames.retain(|&id, _| id >= frame_id);
        had && self.frames.is_empty()
    }
}

impl Default for ChannelReassembler {
    fn default() -> Self { Self::new() }
}

// ─── Адаптивный jitter ───────────────────────────────────────────────────────

/// Адаптивный контроллер задержки jitter-буфера.
/// Порт `ComputeAdaptiveJitterMs` из EvertyGame.
#[derive(Debug, Default)]
pub struct AdaptiveJitter {
    current_ms: u32,
}

impl AdaptiveJitter {
    pub fn new() -> Self { Self { current_ms: 0 } }

    /// Пересчитать задержку по текущим метрикам.
    /// Возвращает новое значение в мс.
    pub fn update(
        &mut self,
        pressure:          crate::evrt::Pressure,
        arrival_delta_ms:  i32,
        backlog_frames:    u32,
        queue_drops:       u64,
        cinema_smooth:     bool,
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

        // Сглаживание: быстро снижаем при critical (нужна скорость),
        // медленно повышаем при Normal.
        if target < self.current_ms {
            self.current_ms = self.current_ms.saturating_sub(
                if pressure == Critical { 4 } else { 1 },
            );
            self.current_ms = self.current_ms.max(target);
        } else if target > self.current_ms {
            self.current_ms = self.current_ms.saturating_add(2).min(target);
        }

        self.current_ms
    }

    pub fn current_ms(&self) -> u32 { self.current_ms }
    pub fn as_duration(&self) -> Duration { Duration::from_millis(self.current_ms as u64) }
}

// ─── Адаптивный relief (хост-сторона) ────────────────────────────────────────
//
// Порт ConsiderAdaptiveRelief из EvertyGame — хост снижает битрейт/FPS
// когда клиент сообщает о критической нагрузке.

/// Состояние адаптивной регулировки на стороне хоста.
#[derive(Debug)]
pub struct AdaptiveRelief {
    enabled:         bool,
    step:            u8,       // 0..=2
    strain_score:    i32,
    recovery_score:  i32,
    last_change_at:  Option<Instant>,
    pending_step:    Option<u8>,
}

impl AdaptiveRelief {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            step:           0,
            strain_score:   0,
            recovery_score: 0,
            last_change_at: None,
            pending_step:   None,
        }
    }

    const COOLDOWN: Duration = Duration::from_millis(1800);

    /// Обработать очередной feedback от клиента.
    /// Возвращает `Some(new_step)` если нужно перенастроить энкодер.
    pub fn on_feedback(
        &mut self,
        fb: &crate::evrt::ReceiverFeedback,
        target_fps: u32,
    ) -> Option<u8> {
        use crate::evrt::Pressure::*;

        if !self.enabled { return None; }

        if let Some(t) = self.last_change_at {
            if t.elapsed() < Self::COOLDOWN { return None; }
        }

        let pressure_critical = fb.pressure == Critical;
        let pressure_high     = fb.pressure == High;
        let decode_behind     = fb.decode_fps > 0
            && fb.decode_fps <= (target_fps.saturating_sub(3)).max(28);
        let decode_collapsed  = fb.decode_fps > 0
            && fb.decode_fps <= (target_fps.saturating_sub(8)).max(24);
        let present_elevated  = fb.present_delta_ms >= 20;
        let present_high      = fb.present_delta_ms >= 26;

        let strained = pressure_critical || pressure_high || decode_behind || present_elevated;
        let severe   = pressure_critical || decode_collapsed || present_high;

        if strained {
            let mut w = 0i32;
            if pressure_critical { w += 3; } else if pressure_high { w += 2; }
            if decode_behind     { w += if decode_collapsed { 3 } else { 2 }; }
            if present_elevated  { w += if present_high     { 2 } else { 1 }; }
            self.strain_score += w.max(1);
            self.recovery_score = 0;
        } else {
            self.recovery_score += 1;
            self.strain_score = (self.strain_score - 1).max(0);
        }

        let threshold = if self.step == 0 { 2 } else if severe { 4 } else { 6 };

        // Ухудшение
        if self.step < 2 && self.strain_score >= threshold {
            let new_step = self.step + 1;
            self.pending_step    = Some(new_step);
            self.strain_score    = 0;
            self.last_change_at  = Some(Instant::now());
            return Some(new_step);
        }

        // Восстановление
        if self.step > 0
            && self.recovery_score >= 10
            && fb.pressure == Normal
            && (fb.decode_fps == 0 || fb.decode_fps >= target_fps.saturating_sub(1))
            && fb.present_delta_ms >= 0 && fb.present_delta_ms <= 16
            && fb.input_estimate_ms >= 0 && fb.input_estimate_ms <= 45
        {
            let new_step = self.step - 1;
            self.pending_step    = Some(new_step);
            self.recovery_score  = 0;
            self.last_change_at  = Some(Instant::now());
            return Some(new_step);
        }

        None
    }

    /// Применить ожидающий шаг и вернуть масштабный коэффициент битрейта.
    pub fn apply_pending(&mut self) -> Option<f32> {
        let step = self.pending_step.take()?;
        self.step = step;
        Some(Self::bitrate_scale(step))
    }

    pub fn current_step(&self) -> u8 { self.step }

    /// Масштаб битрейта для данного шага (по спецификации EvertyGame).
    pub fn bitrate_scale(step: u8) -> f32 {
        match step {
            0 => 1.00,
            1 => 0.88, // High quality: -12%
            _ => 0.80, // Low  quality: -20%
        }
    }
}

// ─── тесты ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefer_latest_drops_old() {
        let q = FrameQueue::new(FrameQueueConfig {
            max_queued_units: 4,
            prefer_latest: true,
            ..Default::default()
        });
        q.enqueue(vec![1], false, 1000);
        q.enqueue(vec![2], false, 2000);
        q.enqueue(vec![3], false, 3000);
        // prefer_latest — только последний должен остаться
        let stats = q.stats();
        assert_eq!(stats.queued_units, 1);
        assert!(stats.dropped_units >= 2);
    }

    #[test]
    fn waiting_for_keyframe_drops_non_idr() {
        let q = FrameQueue::new(FrameQueueConfig::default());
        q.wait_for_keyframe();
        q.enqueue(vec![1], false, 1000); // дропнуть
        q.enqueue(vec![2], false, 2000); // дропнуть
        assert_eq!(q.stats().dropped_units, 2);
        q.enqueue(vec![3], true, 3000); // принять
        assert_eq!(q.stats().queued_units, 1);
    }

    #[test]
    fn hard_reset_on_keyframe() {
        let q = FrameQueue::new(FrameQueueConfig {
            max_queued_units: 8,
            hard_reset_on_keyframe: true,
            prefer_latest: false,
            ..Default::default()
        });
        q.enqueue(vec![1], false, 1000);
        q.enqueue(vec![2], false, 2000);
        assert_eq!(q.stats().queued_units, 2);
        q.enqueue(vec![3], true, 3000); // IDR — должен сбросить старые
        assert_eq!(q.stats().queued_units, 1);
    }

    #[test]
    fn reassembler_single_packet_frame() {
        let mut ch = ChannelReassembler::new();
        ch.set_codec_config(vec![0x00, 0x00, 0x00, 0x01]); // fake SPS

        let raw = crate::evrt::packetize_video_frame(1, 1000, true, &[0xAB, 0xCD]);
        let parsed = crate::evrt::parse(&raw[0], raw[0].len()).unwrap();
        let result = ch.on_packet(&parsed);
        assert!(result.is_some());
        let (bytes, key, _, _) = result.unwrap();
        assert!(key);
        // SPS prepended: [0x00,0x00,0x00,0x01,0xAB,0xCD]
        assert_eq!(&bytes[4..], &[0xAB, 0xCD]);
    }

    #[test]
    fn reassembler_multi_packet_frame() {
        let mut ch = ChannelReassembler::new();
        ch.set_codec_config(vec![0xFF]);
        let payload = vec![0u8; 3000]; // > MAX_PAYLOAD_SIZE
        let packets = crate::evrt::packetize_video_frame(5, 9999, true, &payload);
        assert!(packets.len() > 1);
        let last_idx = packets.len() - 1;
        for (i, raw) in packets.iter().enumerate() {
            let parsed = crate::evrt::parse(raw, raw.len()).unwrap();
            let result = ch.on_packet(&parsed);
            if i < last_idx {
                assert!(result.is_none(), "не должен быть готов до последнего пакета");
            } else {
                assert!(result.is_some(), "должен быть готов после последнего пакета");
            }
        }
    }

    #[test]
    fn adaptive_relief_degrades_on_critical() {
        use crate::evrt::{Pressure, ReceiverFeedback};
        let mut relief = AdaptiveRelief::new(true);
        // Надо перехитрить cooldown — напрямую форсируем last_change_at в прошлое
        // Просто подаём много critical feedback
        let fb = ReceiverFeedback { pressure: Pressure::Critical, decode_fps: 10, ..Default::default() };
        let mut triggered = false;
        for _ in 0..10 {
            if relief.on_feedback(&fb, 60).is_some() {
                triggered = true;
                break;
            }
        }
        assert!(triggered || relief.strain_score > 0);
    }

    #[test]
    fn adaptive_jitter_zero_on_no_pressure() {
        use crate::evrt::Pressure;
        let mut jitter = AdaptiveJitter::new();
        let ms = jitter.update(Pressure::Normal, 5, 0, 0, false);
        assert_eq!(ms, 0);
    }
}
