//! D3D12 Video Encode — аппаратное кодирование напрямую через драйвер,
//! в обход Media Foundation.
//!
//! # Зачем это существует
//!
//! Media Foundation отдаёт HEVC-кодеки только когда в системе установлен
//! пакет «HEVC Video Extensions» из Microsoft Store. Живо подтверждено на
//! Intel UHD 610: `MFTEnumEx` не находит HEVC-энкодера НИ ОДНОГО (проверяем
//! три GUID: `HEVC`, `H265`, `HEVC_ES`, и в аппаратном, и в любом режиме),
//! при том что тот же вызов честно находит `H264(hw)`. То есть Quick Sync
//! работает, драйвер стоит, а HEVC не виден — потому что Microsoft вынесла
//! его из Windows в отдельный платный пакет.
//!
//! Требовать от пользователя ставить что-то из Store — неприемлемо для
//! продукта. NVENC эту проблему обходит тем, что идёт напрямую в драйвер
//! NVIDIA мимо Media Foundation. `ID3D12VideoEncoder` — ровно тот же приём,
//! но стандартный для Windows и одинаковый для ВСЕХ вендоров:
//!
//! | | NVENC SDK | Intel oneVPL | D3D12 Video Encode |
//! |---|---|---|---|
//! | Вендоры | только NVIDIA | только Intel | Intel + AMD + NVIDIA |
//! | SDK для сборки | внешний, ищем в `build.rs` | внешний, надо вендорить | уже в `windows` 0.48 |
//! | Нативный шим | `nvenc_shim.cpp`, ~34 КБ | нужен свой | не нужен, чистый Rust |
//! | Пакет Store | не нужен | не нужен | не нужен |
//!
//! Поэтому выбран D3D12: он покрывает и Intel, и AMD одной реализацией,
//! не тянет ни одного стороннего заголовка и собирается на любой машине.
//!
//! # Ограничения, о которых надо знать честно
//!
//! Требуется Windows 10 20H1+ и драйвер с поддержкой D3D12 Video Encode.
//! На старом драйвере проба вернёт «не поддерживается» — это НЕ ошибка, а
//! штатный ответ, по которому вызывающий код обязан откатиться на свой
//! существующий каскад (NVENC → Media Foundation → OpenH264).
//!
//! Этот модуль сейчас содержит ТОЛЬКО пробу возможностей. Она отвечает на
//! единственный вопрос, ради которого всё затевается: умеет ли конкретная
//! машина кодировать HEVC мимо Media Foundation. Ответ нужен ДО того, как
//! писать сам энкодер, — иначе есть риск потратить дни на путь, который на
//! целевом железе закрыт.

#![allow(dead_code)] // Проба вызывается из host.rs; энкодер — следующий шаг.

/// Что умеет D3D12 Video Encode на этой машине.
#[derive(Clone, Debug, Default)]
pub struct D3d12EncodeStatus {
    /// Устройство D3D12 вообще создалось.
    pub device_ok: bool,
    /// Устройство поддерживает video-encode интерфейс (`ID3D12VideoDevice`).
    pub video_device_ok: bool,
    pub h264: bool,
    pub hevc: bool,
    /// Почему проба не удалась (если не удалась) — для лога, не для UI.
    pub error: Option<String>,
}

impl D3d12EncodeStatus {
    /// Строка для хост-лога. Специально включает и отрицательный результат:
    /// «D3D12 encode: недоступен» — это диагностически ценно ровно настолько
    /// же, насколько положительный ответ.
    pub fn label(&self) -> String {
        if let Some(err) = &self.error {
            return format!("D3D12 encode: недоступен ({err})");
        }
        let mut codecs = Vec::new();
        if self.hevc {
            codecs.push("H265");
        }
        if self.h264 {
            codecs.push("H264");
        }
        if codecs.is_empty() {
            "D3D12 encode: устройство есть, аппаратных кодеков нет".to_owned()
        } else {
            format!("D3D12 encode: {}", codecs.join(", "))
        }
    }

    pub fn has_hevc(&self) -> bool {
        self.hevc
    }
}

#[cfg(windows)]
pub fn d3d12_encode_status() -> &'static D3d12EncodeStatus {
    use std::sync::OnceLock;
    static STATUS: OnceLock<D3d12EncodeStatus> = OnceLock::new();
    STATUS.get_or_init(probe)
}

#[cfg(not(windows))]
pub fn d3d12_encode_status() -> &'static D3d12EncodeStatus {
    use std::sync::OnceLock;
    static STATUS: OnceLock<D3d12EncodeStatus> = OnceLock::new();
    STATUS.get_or_init(|| D3d12EncodeStatus {
        error: Some("не Windows".to_owned()),
        ..Default::default()
    })
}

#[cfg(windows)]
fn probe() -> D3d12EncodeStatus {
    use windows::core::ComInterface;
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Direct3D12::{D3D12CreateDevice, ID3D12Device};
    use windows::Win32::Media::MediaFoundation::{
        ID3D12VideoDevice, D3D12_FEATURE_DATA_VIDEO_ENCODER_CODEC,
        D3D12_FEATURE_VIDEO_ENCODER_CODEC, D3D12_VIDEO_ENCODER_CODEC,
        D3D12_VIDEO_ENCODER_CODEC_H264, D3D12_VIDEO_ENCODER_CODEC_HEVC,
    };

    let mut status = D3d12EncodeStatus::default();

    unsafe {
        // Адаптер по умолчанию (None) — тот же, на котором идёт захват экрана.
        // Специально НЕ перебираем все адаптеры: кодировать надо там же, где
        // лежит захваченная текстура, иначе появится межустройственная копия,
        // а на этих граблях мы уже стояли (см. IDXGIKeyedMutex в capture.rs).
        let mut device: Option<ID3D12Device> = None;
        if let Err(e) = D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut device) {
            status.error = Some(format!("D3D12CreateDevice: {e}"));
            return status;
        }
        let Some(device) = device else {
            status.error = Some("D3D12CreateDevice вернул пустое устройство".to_owned());
            return status;
        };
        status.device_ok = true;

        // Video-encode живёт на отдельном интерфейсе. Драйвер без поддержки
        // видео-кодирования просто не отдаст его — это штатный отказ.
        let video_device: ID3D12VideoDevice = match device.cast::<ID3D12VideoDevice>() {
            Ok(v) => v,
            Err(e) => {
                status.error = Some(format!("ID3D12VideoDevice недоступен: {e}"));
                return status;
            }
        };
        status.video_device_ok = true;

        let check = |codec: D3D12_VIDEO_ENCODER_CODEC| -> bool {
            let mut data = D3D12_FEATURE_DATA_VIDEO_ENCODER_CODEC {
                NodeIndex: 0,
                Codec: codec,
                IsSupported: false.into(),
            };
            let ok = video_device
                .CheckFeatureSupport(
                    D3D12_FEATURE_VIDEO_ENCODER_CODEC,
                    &mut data as *mut _ as *mut _,
                    std::mem::size_of::<D3D12_FEATURE_DATA_VIDEO_ENCODER_CODEC>() as u32,
                )
                .is_ok();
            ok && data.IsSupported.as_bool()
        };

        status.hevc = check(D3D12_VIDEO_ENCODER_CODEC_HEVC);
        status.h264 = check(D3D12_VIDEO_ENCODER_CODEC_H264);
    }

    status
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Проба обязана быть безопасной на ЛЮБОЙ машине, включая CI без GPU:
    /// отсутствие устройства — это заполненный `error`, а не паника.
    #[test]
    fn probe_never_panics_and_reports_a_verdict() {
        let status = d3d12_encode_status();
        // Печатаем вердикт: при запуске тестов на новой машине это самый
        // быстрый способ узнать, что там с аппаратным кодированием.
        // Видно через `cargo test -- --nocapture`.
        eprintln!("[d3d12] {} | {status:?}", status.label());
        // Ровно одно из двух: либо проба дошла до устройства, либо честно
        // объяснила почему нет. Пустого «не знаю» быть не должно.
        assert!(
            status.device_ok || status.error.is_some(),
            "проба должна либо создать устройство, либо назвать причину"
        );
        // Метка не должна быть пустой ни в одном из исходов — её читают в логе.
        assert!(!status.label().is_empty());
    }

    /// Кодек не может считаться поддержанным, если до устройства даже не дошли.
    #[test]
    fn codecs_are_never_reported_without_a_video_device() {
        let status = d3d12_encode_status();
        if !status.video_device_ok {
            assert!(!status.hevc, "HEVC не может быть true без video-устройства");
            assert!(!status.h264, "H264 не может быть true без video-устройства");
        }
    }
}
