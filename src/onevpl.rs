//! Intel oneVPL / Media SDK — проба рантайма.
//!
//! # Зачем отдельная проба до реализации энкодера
//!
//! Media Foundation не отдаёт HEVC без пакета «HEVC Video Extensions» из
//! Microsoft Store (живо подтверждено на Intel UHD 610: `MFTEnumEx` не
//! находит HEVC-энкодера ни по одному из трёх GUID, хотя `H264(hw)` находит).
//! Требовать установку пакета от пользователя нельзя, поэтому нужен путь
//! напрямую в драйвер — как это делает NVENC у NVIDIA. У Intel такой путь
//! называется oneVPL (ранее Intel Media SDK), и его рантайм приезжает ВМЕСТЕ
//! С ДРАЙВЕРОМ, то есть пользователю ставить ничего не надо.
//!
//! Загвоздка в том, что «oneVPL» — это два разных поколения API:
//!
//! * **Legacy Media SDK 1.x** — точка входа `MFXInit`, рантайм
//!   `libmfxhw64.dll` из DriverStore, диспетчер `libmfx.dll`. Это то, что
//!   стоит на старых драйверах, а Gen9.5 (UHD 610, 2017-2018) — как раз
//!   такой случай.
//! * **oneVPL 2.x** — точка входа `MFXLoad`/`MFXCreateSession`, диспетчер
//!   `libvpl.dll`. Новые драйверы и отдельный редистрибутив.
//!
//! Структуры и порядок вызовов у них различаются, поэтому писать энкодер, не
//! зная, какое поколение стоит на целевой машине, — это с приличной
//! вероятностью выкинуть день работы. Проба отвечает на этот вопрос за один
//! запуск.
//!
//! # Почему здесь нет вендорских заголовков
//!
//! Пробе они не нужны. `MFXInit`/`MFXQueryIMPL`/`MFXQueryVersion`/`MFXClose`
//! оперируют только простыми скалярами (`mfxIMPL` = i16, `mfxVersion` = u32,
//! `mfxSession` = непрозрачный указатель, `mfxStatus` = i32), и эти сигнатуры
//! не менялись с Media SDK 1.0. Ошибиться в раскладке структур тут негде.
//!
//! А вот сам энкодер потребует `mfxVideoParam`/`mfxFrameSurface1` — большие
//! структуры с объединениями и `reserved`-полями. Объявлять их по памяти —
//! прямой путь к порче памяти, поэтому под энкодер будут взяты официальные
//! заголовки, а не рукописные определения.

#![allow(dead_code)] // Проба вызывается из host.rs; энкодер — следующий шаг.

/// Что удалось выяснить про oneVPL/MSDK на этой машине.
#[derive(Clone, Debug, Default)]
pub struct OneVplStatus {
    /// Имя DLL, которую удалось загрузить (диспетчер или сам рантайм).
    pub library: Option<String>,
    /// Удалось создать аппаратную сессию.
    pub hardware_session: bool,
    /// Версия API, о которой отчитался рантайм: (Major, Minor).
    pub api_version: Option<(u16, u16)>,
    /// Через что работает реализация — D3D11 / D3D9 / VAAPI / софт.
    pub implementation: Option<String>,
    /// Почему не получилось (если не получилось).
    pub error: Option<String>,
}

impl OneVplStatus {
    /// Строка для хост-лога. Отрицательный результат тоже печатаем — он ровно
    /// так же нужен для решения, что делать дальше.
    pub fn label(&self) -> String {
        if let Some(err) = &self.error {
            return format!("oneVPL/MSDK: недоступен ({err})");
        }
        let lib = self.library.as_deref().unwrap_or("?");
        let ver = self
            .api_version
            .map(|(major, minor)| format!("API {major}.{minor}"))
            .unwrap_or_else(|| "версия неизвестна".to_owned());
        let imp = self.implementation.as_deref().unwrap_or("?");
        let hw = if self.hardware_session {
            "аппаратная сессия ✓"
        } else {
            "только программная"
        };
        format!("oneVPL/MSDK: {lib} · {ver} · {imp} · {hw}")
    }

    pub fn is_hardware_available(&self) -> bool {
        self.hardware_session
    }
}

#[cfg(windows)]
pub fn onevpl_status() -> &'static OneVplStatus {
    use std::sync::OnceLock;
    static STATUS: OnceLock<OneVplStatus> = OnceLock::new();
    STATUS.get_or_init(probe)
}

#[cfg(not(windows))]
pub fn onevpl_status() -> &'static OneVplStatus {
    use std::sync::OnceLock;
    static STATUS: OnceLock<OneVplStatus> = OnceLock::new();
    STATUS.get_or_init(|| OneVplStatus {
        // На Linux аналог — VA-API, это отдельная реализация; здесь честно
        // говорим, что этот путь не про данную платформу.
        error: Some("не Windows".to_owned()),
        ..Default::default()
    })
}

#[cfg(windows)]
mod ffi {
    //! Ровно тот минимум ABI, который нужен пробе. Все типы — скаляры и
    //! непрозрачные указатели, так что раскладку структур перепутать нельзя.

    /// `mfxSession` — непрозрачный указатель на сессию.
    pub type MfxSession = *mut core::ffi::c_void;
    /// `mfxIMPL` — знаковое 16-битное поле с битовыми флагами.
    pub type MfxImpl = i16;
    /// `mfxStatus` — 0 это успех, отрицательные значения — ошибки.
    pub type MfxStatus = i32;

    /// `mfxVersion` — объединение из `{u16 Minor; u16 Major}` и `u32 Version`.
    /// Порядок полей именно такой (Minor первым) — это часть ABI Media SDK.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct MfxVersion {
        pub minor: u16,
        pub major: u16,
    }

    // Значения mfxIMPL, стабильные с Media SDK 1.0.
    pub const MFX_IMPL_HARDWARE_ANY: MfxImpl = 4;
    pub const MFX_IMPL_SOFTWARE: MfxImpl = 1;
    /// «Любая реализация, аппаратная в приоритете» — последний шанс, если
    /// явные запросы аппаратной отвергнуты.
    pub const MFX_IMPL_AUTO_ANY: MfxImpl = 3;
    /// Маска, отделяющая базовую реализацию от транспортных флагов VIA_*.
    pub const MFX_IMPL_BASETYPE_MASK: MfxImpl = 0x00ff;
    pub const MFX_IMPL_VIA_D3D9: MfxImpl = 0x0100;
    pub const MFX_IMPL_VIA_D3D11: MfxImpl = 0x0200;
    pub const MFX_IMPL_VIA_VAAPI: MfxImpl = 0x0400;

    pub const MFX_ERR_NONE: MfxStatus = 0;

    pub type MfxInitFn =
        unsafe extern "C" fn(MfxImpl, *mut MfxVersion, *mut MfxSession) -> MfxStatus;
    pub type MfxQueryImplFn = unsafe extern "C" fn(MfxSession, *mut MfxImpl) -> MfxStatus;
    pub type MfxQueryVersionFn = unsafe extern "C" fn(MfxSession, *mut MfxVersion) -> MfxStatus;
    pub type MfxCloseFn = unsafe extern "C" fn(MfxSession) -> MfxStatus;

    /// Кандидаты на загрузку, в порядке предпочтения.
    ///
    /// `libvpl.dll` — диспетчер oneVPL 2.x. `libmfx.dll` — легаси-диспетчер
    /// Media SDK. `libmfxhw64.dll` — сам рантайм из DriverStore; грузить его
    /// напрямую по имени обычно нельзя (он не в PATH), но попытка ничего не
    /// стоит и иногда срабатывает, если драйвер положил копию рядом.
    pub const CANDIDATES: &[&str] = &["libvpl.dll", "libmfx.dll", "libmfxhw64.dll"];

    pub fn describe_impl(value: MfxImpl) -> String {
        let base = match value & MFX_IMPL_BASETYPE_MASK {
            0 => "auto",
            1 => "software",
            2 | 4 | 5 | 6 | 7 => "hardware",
            _ => "unknown",
        };
        let via = if value & MFX_IMPL_VIA_D3D11 != 0 {
            " via D3D11"
        } else if value & MFX_IMPL_VIA_D3D9 != 0 {
            " via D3D9"
        } else if value & MFX_IMPL_VIA_VAAPI != 0 {
            " via VAAPI"
        } else {
            ""
        };
        format!("{base}{via}")
    }

    pub fn is_hardware(value: MfxImpl) -> bool {
        matches!(value & MFX_IMPL_BASETYPE_MASK, 2 | 4 | 5 | 6 | 7)
    }
}

#[cfg(windows)]
fn probe() -> OneVplStatus {
    use windows::core::PCSTR;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{FreeLibrary, GetProcAddress, LoadLibraryA};

    let mut status = OneVplStatus::default();
    // Копим ВСЕ неудачи, а не только последнюю: на чужой машине, которую я не
    // могу отладить, полный список «что пробовали и что ответило» — это и есть
    // диагностика. Один код ошибки без контекста заставляет гадать.
    let mut tried: Vec<String> = Vec::new();

    // Наличие самого рантайма Intel проверяем отдельно от диспетчера: если
    // libmfxhw64.dll нет нигде, то никакой диспетчер помочь не сможет, и это
    // сразу закрывает вопрос — драйвер медиа-часть не поставил.
    let runtime_found = find_intel_runtime();

    for name in ffi::CANDIDATES {
        let c_name = format!("{name}\0");
        let module: HMODULE = match unsafe { LoadLibraryA(PCSTR(c_name.as_ptr())) } {
            Ok(m) => {
                if m.is_invalid() {
                    tried.push(format!("{name}=не загрузилась"));
                    continue;
                }
                m
            }
            Err(_) => {
                tried.push(format!("{name}=не загрузилась"));
                continue;
            }
        };

        // Загрузилась — достаём точки входа. Отсутствие MFXInit означает,
        // что это не тот DLL (или сборка oneVPL без legacy-совместимости).
        let init = unsafe { GetProcAddress(module, PCSTR(b"MFXInit\0".as_ptr())) };
        let Some(init) = init else {
            unsafe {
                let _ = FreeLibrary(module);
            }
            tried.push(format!("{name}=нет MFXInit"));
            continue;
        };
        let init: ffi::MfxInitFn = unsafe { std::mem::transmute(init) };

        // Перебираем варианты реализации, а не один. Разные поколения
        // рантайма отвечают на разные запросы: диспетчер oneVPL 2.x может
        // отказать на «просто аппаратную», но согласиться, когда явно указан
        // транспорт D3D11, и наоборот. Просить минимальную версию 1.0 важно —
        // рантайм отдаёт реализацию НЕ НИЖЕ запрошенной, так что запрос 1.0
        // принимают и легаси-MSDK, и oneVPL 2.x, а запрос 2.0 отсёк бы
        // легаси-рантайм, который на Gen9.5 как раз и стоит.
        let attempts: [(ffi::MfxImpl, &str); 4] = [
            (ffi::MFX_IMPL_HARDWARE_ANY, "HARDWARE_ANY"),
            (
                ffi::MFX_IMPL_HARDWARE_ANY | ffi::MFX_IMPL_VIA_D3D11,
                "HARDWARE_ANY|VIA_D3D11",
            ),
            (
                ffi::MFX_IMPL_HARDWARE_ANY | ffi::MFX_IMPL_VIA_D3D9,
                "HARDWARE_ANY|VIA_D3D9",
            ),
            (ffi::MFX_IMPL_AUTO_ANY, "AUTO_ANY"),
        ];

        let mut session: ffi::MfxSession = std::ptr::null_mut();
        let mut opened_with = None;
        for (impl_value, impl_name) in attempts {
            let mut version = ffi::MfxVersion { minor: 0, major: 1 };
            let mut candidate: ffi::MfxSession = std::ptr::null_mut();
            let rc = unsafe { init(impl_value, &mut version, &mut candidate) };
            if rc == ffi::MFX_ERR_NONE && !candidate.is_null() {
                session = candidate;
                opened_with = Some(impl_name);
                break;
            }
            tried.push(format!("{name}/{impl_name}=MFXInit({rc})"));
        }

        let Some(opened_with) = opened_with else {
            // Эта библиотека не дала сессии — идём к СЛЕДУЮЩЕЙ. Первая версия
            // пробы здесь возвращалась наружу, и из-за этого на живой машине
            // `libmfx.dll` (легаси-диспетчер, который как раз и умеет старый
            // рантайм Gen9.5) не проверялся вообще: `libvpl.dll` грузился
            // первым, отвечал MFX_ERR_UNSUPPORTED — и проба сдавалась.
            unsafe {
                let _ = FreeLibrary(module);
            }
            continue;
        };

        status.library = Some(format!("{name} ({opened_with})"));
        status.hardware_session = true;

        // Реальную версию спрашиваем у рантайма: то, что мы передали в
        // MFXInit — лишь минимально требуемая, а не фактическая.
        if let Some(query_version) =
            unsafe { GetProcAddress(module, PCSTR(b"MFXQueryVersion\0".as_ptr())) }
        {
            let query_version: ffi::MfxQueryVersionFn =
                unsafe { std::mem::transmute(query_version) };
            let mut actual = ffi::MfxVersion::default();
            if unsafe { query_version(session, &mut actual) } == ffi::MFX_ERR_NONE {
                status.api_version = Some((actual.major, actual.minor));
            }
        }

        if let Some(query_impl) =
            unsafe { GetProcAddress(module, PCSTR(b"MFXQueryIMPL\0".as_ptr())) }
        {
            let query_impl: ffi::MfxQueryImplFn = unsafe { std::mem::transmute(query_impl) };
            let mut value: ffi::MfxImpl = 0;
            if unsafe { query_impl(session, &mut value) } == ffi::MFX_ERR_NONE {
                status.implementation = Some(ffi::describe_impl(value));
                status.hardware_session = ffi::is_hardware(value);
            }
        }

        if let Some(close) = unsafe { GetProcAddress(module, PCSTR(b"MFXClose\0".as_ptr())) } {
            let close: ffi::MfxCloseFn = unsafe { std::mem::transmute(close) };
            unsafe {
                close(session);
            }
        }
        // Модуль намеренно НЕ выгружаем: рантайм Intel держит внутреннее
        // состояние, и повторная загрузка/выгрузка в одном процессе — известный
        // источник проблем. Проба выполняется один раз за процесс (OnceLock),
        // так что утечки одного HMODULE тут нет — он нужен и дальше, когда
        // появится настоящий энкодер.
        return status;
    }

    // Ни один диспетчер не дал сессии. Формулировка вывода зависит от того,
    // есть ли вообще рантайм Intel: «диспетчер не смог» и «драйвер не поставил
    // медиа-часть» — это разные диагнозы с разными следующими шагами.
    status.error = Some(match &runtime_found {
        Some(path) => format!(
            "рантайм Intel НАЙДЕН ({path}), но ни один диспетчер не открыл сессию: {}",
            tried.join("; ")
        ),
        None => format!(
            "рантайм Intel (libmfxhw64.dll) не найден ни в System32, ни в DriverStore — \
             драйвер не установил медиа-часть; попытки: {}",
            tried.join("; ")
        ),
    });
    status
}

/// Найти сам рантайм Intel (не диспетчер). Диспетчер — это тонкая прослойка,
/// которая ищет вот этот файл; если его нет, дальше идти некуда.
///
/// Ищем и в System32 (куда его иногда кладут), и в DriverStore (штатное место
/// для файлов, приехавших с драйвером).
#[cfg(windows)]
fn find_intel_runtime() -> Option<String> {
    const NAMES: &[&str] = &["libmfxhw64.dll", "libmfxhw32.dll"];

    for dir in ["C:\\Windows\\System32", "C:\\Windows\\SysWOW64"] {
        for name in NAMES {
            let path = std::path::Path::new(dir).join(name);
            if path.exists() {
                return Some(path.display().to_string());
            }
        }
    }

    // DriverStore: путей много и они длинные, поэтому обходим только два
    // уровня — этого достаточно, файлы драйвера лежат неглубоко, а полный
    // рекурсивный обход этой папки заметно тормозит старт.
    let repo = std::path::Path::new("C:\\Windows\\System32\\DriverStore\\FileRepository");
    let Ok(entries) = std::fs::read_dir(repo) else {
        return None;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        for name in NAMES {
            let candidate = dir.join(name);
            if candidate.exists() {
                return Some(candidate.display().to_string());
            }
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Энкодер
// ─────────────────────────────────────────────────────────────────────────────

/// Аппаратный энкодер Intel поверх шима `src/onevpl_shim.cpp`.
///
/// Возвращает тот же `NvencPacket`, что NVENC и Media Foundation, поэтому весь
/// код ниже по конвейеру (пакетизация, отправка, кэш codec_config) остаётся
/// нетронутым и кодек-агностичным.
#[cfg(all(windows, onevpl_ffi))]
pub struct OneVplEncoder {
    ctx: *mut std::ffi::c_void,
    codec: crate::nvenc::NvencCodec,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
}

// Контекст принадлежит ровно одному владельцу и не разделяется между потоками
// без внешней синхронизации — как и `NvencEncoder`. `Send` нужен, чтобы
// энкодер можно было держать в потоке кодирования.
#[cfg(all(windows, onevpl_ffi))]
unsafe impl Send for OneVplEncoder {}

#[cfg(all(windows, onevpl_ffi))]
impl OneVplEncoder {
    pub fn new(
        codec: crate::nvenc::NvencCodec,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
    ) -> Result<Self, String> {
        // Чётные размеры обязательны: NV12 хранит цветность в половинном
        // разрешении, нечётная сторона просто не представима.
        let width = width.max(2) & !1;
        let height = height.max(2) & !1;
        let fps = fps.clamp(5, 60);
        let bitrate = bitrate.max(500_000);

        let mut ctx: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut err = [0u8; 256];
        let rc = unsafe {
            shim::everty_onevpl_create(
                codec_to_shim(codec),
                width,
                height,
                fps,
                bitrate,
                &mut ctx,
                err.as_mut_ptr() as *mut std::ffi::c_char,
                err.len(),
            )
        };
        if rc != 0 || ctx.is_null() {
            return Err(read_err(&err));
        }
        Ok(Self {
            ctx,
            codec,
            width,
            height,
            fps,
            bitrate,
        })
    }

    pub fn matches(
        &self,
        codec: crate::nvenc::NvencCodec,
        width: u32,
        height: u32,
        fps: u32,
    ) -> bool {
        self.codec == codec
            && self.width == (width.max(2) & !1)
            && self.height == (height.max(2) & !1)
            && self.fps == fps.clamp(5, 60)
    }

    pub fn current_bitrate(&self) -> u32 {
        self.bitrate
    }

    pub fn encode_bgra(
        &mut self,
        bgra: &[u8],
        force_key: bool,
    ) -> Result<Option<crate::nvenc::NvencPacket>, String> {
        let mut data: *const u8 = std::ptr::null();
        let mut len: usize = 0;
        let mut key: i32 = 0;
        let mut err = [0u8; 256];
        let rc = unsafe {
            shim::everty_onevpl_encode(
                self.ctx,
                bgra.as_ptr(),
                bgra.len(),
                i32::from(force_key),
                &mut data,
                &mut len,
                &mut key,
                err.as_mut_ptr() as *mut std::ffi::c_char,
                err.len(),
            )
        };
        if rc != 0 {
            return Err(read_err(&err));
        }
        if data.is_null() || len == 0 {
            // Рантайм принял кадр, но пакета пока нет — штатный ответ, не сбой.
            return Ok(None);
        }
        let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();

        // Диагностический дамп: EVERTYDESK_ONEVPL_DUMP=<путь> кладёт сырой
        // битстрим в файл. Нужен потому, что клиентский HEVC-декодер Windows
        // на плохом потоке не возвращает ошибку, а роняет процесс — по факту
        // падения нельзя понять, ЧТО именно в потоке не так. Разбор реальных
        // NAL-заголовков отвечает на это точно, в отличие от догадок.
        if let Ok(path) = std::env::var("EVERTYDESK_ONEVPL_DUMP") {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = f.write_all(&bytes);
            }
        }

        Ok(Some(crate::nvenc::NvencPacket {
            codec: self.codec,
            bytes,
            key: key != 0,
        }))
    }
}

#[cfg(all(windows, onevpl_ffi))]
impl Drop for OneVplEncoder {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe { shim::everty_onevpl_destroy(self.ctx) };
            self.ctx = std::ptr::null_mut();
        }
    }
}

/// Какие кодеки реально может закодировать железо этой машины.
///
/// Отвечает сам рантайм через `MFXVideoENCODE_Query`, а не наши догадки по
/// модели GPU — на Gen9.5 это принципиально, там HEVC есть в кремнии, но его
/// наличие в конкретной сборке рантайма надо проверять.
#[cfg(all(windows, onevpl_ffi))]
pub fn onevpl_encoder_codecs() -> Vec<crate::nvenc::NvencCodec> {
    use std::sync::OnceLock;
    static CODECS: OnceLock<Vec<crate::nvenc::NvencCodec>> = OnceLock::new();
    CODECS
        .get_or_init(|| {
            let mut mask: u32 = 0;
            let mut err = [0u8; 256];
            let rc = unsafe {
                shim::everty_onevpl_supported_codecs(
                    &mut mask,
                    err.as_mut_ptr() as *mut std::ffi::c_char,
                    err.len(),
                )
            };
            if rc != 0 {
                return Vec::new();
            }
            let mut out = Vec::new();
            // H265 первым: при равной доступности он предпочтительнее по
            // качеству на мегабит, а ради него всё и затевалось.
            if mask & 0x2 != 0 {
                out.push(crate::nvenc::NvencCodec::H265);
            }
            if mask & 0x1 != 0 {
                out.push(crate::nvenc::NvencCodec::H264);
            }
            out
        })
        .clone()
}

#[cfg(not(all(windows, onevpl_ffi)))]
pub fn onevpl_encoder_codecs() -> Vec<crate::nvenc::NvencCodec> {
    Vec::new()
}

/// Заглушка для платформ и сборок без oneVPL.
///
/// Существует, чтобы каскад энкодеров в `host.rs` не пришлось обвешивать
/// `#[cfg]`: там просто вызывается `new()`, который здесь всегда отказывает, и
/// управление уходит к следующему бэкенду.
#[cfg(not(all(windows, onevpl_ffi)))]
pub struct OneVplEncoder {
    _private: (),
}

#[cfg(not(all(windows, onevpl_ffi)))]
impl OneVplEncoder {
    pub fn new(
        _codec: crate::nvenc::NvencCodec,
        _width: u32,
        _height: u32,
        _fps: u32,
        _bitrate: u32,
    ) -> Result<Self, String> {
        Err("oneVPL недоступен в этой сборке".to_owned())
    }

    pub fn matches(
        &self,
        _codec: crate::nvenc::NvencCodec,
        _width: u32,
        _height: u32,
        _fps: u32,
    ) -> bool {
        false
    }

    pub fn current_bitrate(&self) -> u32 {
        0
    }

    pub fn encode_bgra(
        &mut self,
        _bgra: &[u8],
        _force_key: bool,
    ) -> Result<Option<crate::nvenc::NvencPacket>, String> {
        Err("oneVPL недоступен в этой сборке".to_owned())
    }
}

#[cfg(all(windows, onevpl_ffi))]
fn codec_to_shim(codec: crate::nvenc::NvencCodec) -> i32 {
    match codec {
        crate::nvenc::NvencCodec::H265 => 1,
        // AV1 у legacy MSDK нет; шим трактует всё прочее как H264, и это
        // честнее, чем притворяться, что AV1 поддержан.
        _ => 0,
    }
}

#[cfg(all(windows, onevpl_ffi))]
fn read_err(buf: &[u8]) -> String {
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

#[cfg(all(windows, onevpl_ffi))]
mod shim {
    use std::ffi::{c_char, c_void};

    #[link(name = "everty_onevpl_shim", kind = "static")]
    extern "C" {
        pub fn everty_onevpl_supported_codecs(
            mask: *mut u32,
            err: *mut c_char,
            err_len: usize,
        ) -> i32;
        pub fn everty_onevpl_create(
            codec: i32,
            width: u32,
            height: u32,
            fps: u32,
            bitrate: u32,
            out_ctx: *mut *mut c_void,
            err: *mut c_char,
            err_len: usize,
        ) -> i32;
        pub fn everty_onevpl_encode(
            ctx: *mut c_void,
            bgra: *const u8,
            bgra_len: usize,
            force_key: i32,
            out_data: *mut *const u8,
            out_len: *mut usize,
            out_key: *mut i32,
            err: *mut c_char,
            err_len: usize,
        ) -> i32;
        pub fn everty_onevpl_destroy(ctx: *mut c_void);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// На машине без Intel-графики перечисление кодеков обязано вернуть пустой
    /// список, а не паниковать и не выдумывать поддержку.
    #[test]
    fn codec_enumeration_is_safe_without_intel_hardware() {
        let codecs = onevpl_encoder_codecs();
        eprintln!("[onevpl] кодеки: {codecs:?}");
        if onevpl_status().library.is_none() {
            assert!(
                codecs.is_empty(),
                "без рантайма Intel кодеков быть не может"
            );
        }
    }

    /// Проба обязана быть безопасной на любой машине, включая эту (RTX, без
    /// Intel-графики): отсутствие рантайма — заполненный `error`, не паника.
    #[test]
    fn probe_never_panics_and_reports_a_verdict() {
        let status = onevpl_status();
        eprintln!("[onevpl] {} | {status:?}", status.label());
        assert!(
            status.library.is_some() || status.error.is_some(),
            "проба должна либо назвать библиотеку, либо объяснить отказ"
        );
        assert!(!status.label().is_empty());
    }

    /// Без загруженной библиотеки аппаратная сессия объявлена быть не может.
    #[test]
    fn hardware_is_never_claimed_without_a_library() {
        let status = onevpl_status();
        if status.library.is_none() {
            assert!(!status.hardware_session);
            assert!(status.api_version.is_none());
        }
    }

    #[test]
    fn impl_flags_decode_to_readable_transport() {
        use super::ffi::*;
        assert!(is_hardware(MFX_IMPL_HARDWARE_ANY));
        assert!(!is_hardware(MFX_IMPL_SOFTWARE));
        assert_eq!(
            describe_impl(MFX_IMPL_HARDWARE_ANY | MFX_IMPL_VIA_D3D11),
            "hardware via D3D11"
        );
        assert_eq!(describe_impl(MFX_IMPL_SOFTWARE), "software");
    }
}
